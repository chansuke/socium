pub use crate::agent::state::{
    load_json_file as load_local_state, save_json_file as save_local_state, workspace_state_path,
};
pub mod activity;
mod activity_log;
mod control;
mod execution;
mod models;
mod process;
pub use control::TaskControl;
pub use execution::SANDBOX_DESCRIPTION;
pub mod agent;
pub mod constants;
pub mod error;
pub mod providers;
pub mod tools;

pub use crate::activity::{ActivityEvent, ActivityKind, ThoughtRole};
pub use crate::agent::context::ContextMetrics;
pub use crate::error::AgentError;

use crate::activity::ActivityClock;
use crate::activity_log::{ACTIVITY_LOG_FILE_NAME, ActivityRecorder};
use crate::agent::compaction::{
    CompactionRequest, compaction_threshold_tokens, retention_boundary, summarize_with_llm,
};
use crate::agent::context::{
    context_metrics_for_input, extend_summary_text, history_input_tokens, prepare_context_messages,
    resolve_context_budget, summarize_messages,
};
use crate::agent::state::{
    ConversationState, ConversationStateRef, ConversationSummary, ConversationSummaryState,
    load_json_file, save_json_file,
};
use crate::agent::turn::{TurnContext, run_agent_turn};
use crate::agent::types::{Message, strip_thinking};
use crate::constants::*;
use crate::models::{ModelConfig, ModelRegistry};
use crate::providers::error::ProviderError;
use crate::providers::{Provider, TokenUsage, build_provider};
use crate::tools::{ToolDefinition, tools_for_model};
use std::env;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Reads configuration and API keys. Injected so tests never touch the
/// process environment.
type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// A registered model as `/model` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub alias: String,
    pub provider: String,
    pub model_id: String,
    /// One line for the picker, from `description` in models.toml.
    pub description: Option<String>,
    pub active: bool,
    /// Why the model cannot be selected now, such as a missing API key.
    pub unavailable: Option<String>,
}

/// Result of `/model <alias>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSwitch {
    pub model: ModelChoice,
    /// Settings the new model ignores, a compaction model that could not
    /// be reached, or a history too large for the new model.
    pub warnings: Vec<String>,
    /// The history reaches the new model's compaction threshold; compact
    /// before the next turn instead of trimming it.
    pub needs_compaction: bool,
}

/// Everything that changes together when the model changes.
struct ModelTarget {
    model: ModelConfig,
    provider: Box<dyn Provider>,
    /// `None` summarizes with `provider`.
    compaction_provider: Option<Box<dyn Provider>>,
    compaction_model: String,
    warnings: Vec<String>,
}

impl ModelTarget {
    fn build(
        registry: &ModelRegistry,
        model: ModelConfig,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ProviderError> {
        let provider_config = registry.provider(&model.provider).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("Unknown provider '{}'", model.provider),
            )
        })?;
        let provider = build_provider(provider_config, &model, lookup)?;
        let mut warnings = Vec::new();
        let compact = model
            .compact_model
            .as_deref()
            .filter(|alias| *alias != model.alias)
            .and_then(|alias| registry.find(alias));
        let (compaction_provider, compaction_model) = match compact {
            None => (None, model.id.clone()),
            Some(compact) => match registry
                .provider(&compact.provider)
                .ok_or_else(|| format!("unknown provider '{}'", compact.provider))
                .and_then(|config| {
                    build_provider(config, compact, lookup).map_err(|err| err.to_string())
                }) {
                Ok(provider) => (Some(provider), compact.id.clone()),
                Err(err) => {
                    warnings.push(format!(
                        "Compaction model '{}' is unavailable ({err}); summarizing with '{}'",
                        compact.alias, model.alias
                    ));
                    (None, model.id.clone())
                }
            },
        };
        Ok(Self {
            model,
            provider,
            compaction_provider,
            compaction_model,
            warnings,
        })
    }
}

pub struct AgentReply {
    pub text: String,
}

/// Outcome of a compaction run: how much history the summary now covers
/// and whether the LLM summarizer succeeded or the deterministic fallback
/// was used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionReport {
    pub covered_messages: usize,
    pub retained_messages: usize,
    pub used_llm: bool,
    pub metrics: ContextMetrics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    pub role: TranscriptRole,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProgress {
    ModelRequestStarted {
        round: usize,
        provider: String,
    },
    ApprovalRequested {
        request_id: u64,
        tool: String,
        /// The tool's main argument on one line, such as the command.
        summary: String,
        input: String,
    },
    ApprovalRequired {
        tool: String,
        reason: String,
    },
    /// The human answered, the request timed out, or the task was cancelled.
    ApprovalResolved {
        request_id: u64,
        approved: bool,
    },
    /// Structured record of the turn for the Activity pane.
    Activity(ActivityEvent),
    /// One provider stream event as received. Shown on request, never saved.
    RawProviderEvent {
        turn_id: usize,
        round: usize,
        json: String,
    },
    AssistantText {
        text: String,
    },
    ThinkingText {
        text: String,
    },
    ToolCallStarted {
        name: String,
    },
    ToolCallFinished {
        name: String,
        is_error: bool,
    },
    CommandOutput {
        stdout_bytes: u64,
        stderr_bytes: u64,
    },
    CommandStarted {
        command: String,
        cwd: String,
    },
    CommandFinished {
        command: String,
        exit_code: Option<i32>,
        timed_out: bool,
        stdout_preview: String,
        stderr_preview: String,
    },
    CompactionStarted,
    CompactionFinished {
        covered_messages: usize,
        used_llm: bool,
    },
}

pub struct AgentService {
    control: TaskControl,
    state_lock: Option<std::fs::File>,
    workspace_root: PathBuf,
    conversation_path: PathBuf,
    messages: Vec<Message>,
    events: Vec<serde_json::Value>,
    summary_state: ConversationSummaryState,
    registry: ModelRegistry,
    active_model: ModelConfig,
    /// Alias chosen with `/model` and saved with the conversation. `None`
    /// leaves the choice to `MODEL` and the registry default.
    selected_model: Option<String>,
    provider: Box<dyn Provider>,
    compaction_provider: Option<Box<dyn Provider>>,
    compaction_model: String,
    env: EnvLookup,
    /// `MAX_TOKENS`, used when the model sets no `max_output_tokens`.
    max_tokens_override: Option<u32>,
    max_tokens: u32,
    tools: Vec<ToolDefinition>,
    client: reqwest::Client,
    /// Configuration problems found while choosing the startup model.
    warnings: Vec<String>,
    /// Usage measured on the most recent model call. Volatile: not
    /// persisted, so the first turn after a restart falls back to the
    /// heuristic estimate.
    last_measured_usage: Option<TokenUsage>,
}

fn provider_http_client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(ProviderError::Http)
}

impl AgentService {
    pub fn from_current_dir() -> Result<Self, AgentError> {
        Self::from_env(env::current_dir().map_err(AgentError::State)?)
    }

    /// Construction seam that takes the provider and HTTP client directly,
    /// so tests can inject a mock `Provider` without touching the
    /// environment or the network.
    #[cfg(test)]
    pub(crate) fn with_provider(
        workspace_root: PathBuf,
        provider: Box<dyn Provider>,
        model: String,
        max_tokens: u32,
        client: reqwest::Client,
    ) -> Self {
        let target = ModelTarget {
            model: ModelConfig::ad_hoc(&model),
            provider,
            compaction_provider: None,
            compaction_model: DEFAULT_COMPACTION_MODEL.to_string(),
            warnings: Vec::new(),
        };
        Self::new(
            workspace_root,
            ModelRegistry::builtin(),
            target,
            Arc::new(|_| None),
            Some(max_tokens),
            client,
        )
    }

    fn new(
        workspace_root: PathBuf,
        registry: ModelRegistry,
        target: ModelTarget,
        env: EnvLookup,
        max_tokens_override: Option<u32>,
        client: reqwest::Client,
    ) -> Self {
        let conversation_path = workspace_root
            .join(STATE_DIR_NAME)
            .join(CONVERSATION_STATE_FILE_NAME);
        let mut service = Self {
            control: TaskControl::default(),
            state_lock: None,
            workspace_root,
            conversation_path,
            messages: Vec::new(),
            events: Vec::new(),
            summary_state: ConversationSummaryState::default(),
            registry,
            active_model: target.model,
            selected_model: None,
            provider: target.provider,
            compaction_provider: target.compaction_provider,
            compaction_model: target.compaction_model,
            env,
            max_tokens_override,
            max_tokens: DEFAULT_MAX_TOKENS,
            tools: Vec::new(),
            client,
            warnings: target.warnings,
            last_measured_usage: None,
        };
        service.apply_model_limits();
        service
    }

    pub fn from_env(workspace_root: PathBuf) -> Result<Self, AgentError> {
        let lookup: EnvLookup = Arc::new(|key| env::var(key).ok());
        let registry = ModelRegistry::load(lookup.as_ref())?;
        let max_tokens_override = lookup("MAX_TOKENS")
            .map(|raw| {
                raw.parse::<u32>().map_err(|e| {
                    Error::new(
                        ErrorKind::InvalidInput,
                        format!("MAX_TOKENS must be an unsigned integer: {e}"),
                    )
                })
            })
            .transpose()?;
        let client = provider_http_client()?;

        // Check actual enforcement before advertising a usable agent. A missing
        // backend or disabled namespaces is a startup error, never a downgrade.
        let probe = crate::process::run(
            &["true".into()],
            &workspace_root,
            Duration::from_secs(5),
            &TaskControl::default(),
        )
        .map_err(|e| Error::other(format!("OS sandbox initialization failed: {e}")))?;
        if !probe.status.success() || probe.timed_out {
            return Err(Error::other(format!(
                "OS sandbox initialization failed; no host fallback: {}",
                String::from_utf8_lossy(&probe.stderr)
            ))
            .into());
        }

        Self::open(
            workspace_root,
            registry,
            lookup,
            max_tokens_override,
            client,
        )
    }

    /// Restores the workspace conversation and selects its model: the
    /// alias saved with `/model`, then `MODEL`, then the registry default.
    /// A candidate that cannot be built is skipped with a warning; only the
    /// last one is fatal. History is stripped of thinking when the model
    /// differs from the saved one.
    pub(crate) fn open(
        workspace_root: PathBuf,
        registry: ModelRegistry,
        lookup: EnvLookup,
        max_tokens_override: Option<u32>,
        client: reqwest::Client,
    ) -> Result<Self, AgentError> {
        let state_lock = crate::agent::state::lock_workspace(&workspace_root)?;
        let state = load_json_file::<ConversationState>(&workspace_state_path(
            &workspace_root,
            CONVERSATION_STATE_FILE_NAME,
        )?)?;

        let mut warnings = Vec::new();
        if lookup("COMPACT_MODEL").is_some() {
            warnings.push(
                "COMPACT_MODEL is no longer supported and is ignored; set compact_model for the model in models.toml"
                    .to_string(),
            );
        }
        let saved = state.active_model.clone();
        let mut candidates = Vec::new();
        if let Some(alias) = &saved {
            match registry.find(alias) {
                Some(model) => candidates.push(model.clone()),
                None => warnings.push(format!(
                    "Saved model '{alias}' is no longer registered; using the default"
                )),
            }
        }
        if let Some(value) = lookup("MODEL").filter(|value| !value.trim().is_empty()) {
            candidates.push(registry.resolve_env_model(value.trim()));
        }
        candidates.push(registry.default_model().clone());
        candidates.dedup();

        // When none of the preferred models can start, such as without
        // ANTHROPIC_API_KEY, start on the first registered model that can.
        let preferred = candidates.len();
        for model in &registry.models {
            if !candidates.contains(model) {
                candidates.push(model.clone());
            }
        }
        let mut target = None;
        let mut failures = Vec::new();
        for (index, model) in candidates.into_iter().enumerate() {
            let alias = model.alias.clone();
            match ModelTarget::build(&registry, model, lookup.as_ref()) {
                Ok(built) => {
                    if index < preferred {
                        warnings.extend(
                            failures
                                .iter()
                                .map(|failure| format!("Model {failure}; using '{alias}'")),
                        );
                    } else {
                        warnings.push(format!(
                            "No preferred model is available ({}); started with '{alias}'",
                            failures.join("; ")
                        ));
                    }
                    target = Some(built);
                    break;
                }
                Err(err) if index < preferred => {
                    failures.push(format!("'{alias}' is unavailable: {err}"))
                }
                Err(_) => {}
            }
        }
        let Some(mut target) = target else {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!(
                    "No registered model can start ({}); set the API key of one of them",
                    failures.join("; ")
                ),
            )
            .into());
        };
        warnings.append(&mut target.warnings);
        target.warnings = warnings;

        let mut service = Self::new(
            workspace_root,
            registry,
            target,
            lookup,
            max_tokens_override,
            client,
        );
        service.state_lock = Some(state_lock);
        service.apply_state(state)?;
        if let Some(alias) = saved {
            if alias == service.active_model.alias {
                service.selected_model = Some(alias);
            } else {
                strip_thinking(&mut service.messages);
            }
        }
        service.persist_state()?;
        Ok(service)
    }

    /// Makes `target` current and returns the previous one, so a failed
    /// switch can be undone.
    fn install(&mut self, mut target: ModelTarget) -> ModelTarget {
        std::mem::swap(&mut self.active_model, &mut target.model);
        std::mem::swap(&mut self.provider, &mut target.provider);
        std::mem::swap(
            &mut self.compaction_provider,
            &mut target.compaction_provider,
        );
        std::mem::swap(&mut self.compaction_model, &mut target.compaction_model);
        self.apply_model_limits();
        target
    }

    fn apply_model_limits(&mut self) {
        self.max_tokens = self
            .active_model
            .max_output_tokens
            .or(self.max_tokens_override)
            .unwrap_or(DEFAULT_MAX_TOKENS);
        self.tools = tools_for_model(self.active_model.features.anthropic_text_editor);
    }

    fn model_choice(&self, model: &ModelConfig) -> ModelChoice {
        let unavailable = match self.registry.provider(&model.provider) {
            None => Some(format!("unknown provider '{}'", model.provider)),
            Some(provider) => (self.env)(&provider.api_key_env)
                .filter(|key| !key.is_empty())
                .is_none()
                .then(|| format!("{} is not set", provider.api_key_env)),
        };
        ModelChoice {
            alias: model.alias.clone(),
            provider: model.provider.clone(),
            model_id: model.id.clone(),
            description: model.description.clone(),
            active: *model == self.active_model,
            unavailable,
        }
    }

    pub fn active_model(&self) -> ModelChoice {
        let mut choice = self.model_choice(&self.active_model);
        choice.unavailable = None;
        choice
    }

    /// Registered models in registry order, plus the current model when it
    /// came from `MODEL` rather than the registry.
    pub fn available_models(&self) -> Vec<ModelChoice> {
        let mut choices = self
            .registry
            .models
            .iter()
            .map(|model| self.model_choice(model))
            .collect::<Vec<_>>();
        if !self.registry.is_registered(&self.active_model) {
            choices.push(self.active_model());
        }
        choices
    }

    /// Switches the conversation to a registered model between turns. The
    /// choice is saved with the conversation, and thinking from the
    /// previous model is removed from history.
    pub fn switch_model(&mut self, alias: &str) -> Result<ModelSwitch, AgentError> {
        self.ensure_state_lock()?;
        let model = self.registry.find(alias).cloned().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("Unknown model '{alias}'; type /model to list models"),
            )
        })?;
        if model == self.active_model {
            // Choosing the current model still records the choice.
            if self.selected_model.as_deref() != Some(alias) {
                let previous = self.selected_model.replace(alias.to_string());
                if let Err(err) = self.persist_state() {
                    self.selected_model = previous;
                    return Err(err.into());
                }
            }
            return Ok(ModelSwitch {
                model: self.active_model(),
                warnings: Vec::new(),
                needs_compaction: false,
            });
        }
        let mut target = ModelTarget::build(&self.registry, model, self.env.as_ref())?;
        let mut warnings = std::mem::take(&mut target.warnings);
        let original_messages = self.messages.clone();
        strip_thinking(&mut self.messages);
        let previous = self.install(target);
        let previous_selection = self.selected_model.replace(alias.to_string());
        if let Err(err) = self.persist_state() {
            self.messages = original_messages;
            self.install(previous);
            self.selected_model = previous_selection;
            return Err(err.into());
        }
        // The last measurement described another model's prompt.
        self.last_measured_usage = None;
        warnings.extend(self.provider.startup_warnings());
        let needs_compaction = match self.history_pressure()? {
            Some((history, budget)) => {
                warnings.push(format!(
                    "History is about {history} tokens, near or over {alias}'s input budget of {budget}; compact it before continuing"
                ));
                true
            }
            None => false,
        };
        Ok(ModelSwitch {
            model: self.active_model(),
            warnings,
            needs_compaction,
        })
    }

    /// The estimated history and input budget when the history reaches the
    /// compaction threshold of the current model.
    fn history_pressure(&self) -> std::io::Result<Option<(u32, u32)>> {
        let budget = resolve_context_budget(self.active_model.context_window, self.max_tokens)?;
        let history = history_input_tokens(
            &self.messages,
            &self.summary_state,
            &self.active_model.id,
            budget,
            &self.tools,
        );
        let input_budget = budget.input_budget_tokens();
        Ok((history >= compaction_threshold_tokens(input_budget))
            .then_some((history, input_budget)))
    }

    pub fn task_control(&self) -> TaskControl {
        self.control.clone()
    }

    fn ensure_state_lock(&mut self) -> std::io::Result<()> {
        if self.state_lock.is_none() {
            self.state_lock = Some(crate::agent::state::lock_workspace(&self.workspace_root)?);
        }
        Ok(())
    }

    #[cfg(test)]
    fn restore_state(&mut self) -> std::io::Result<()> {
        self.ensure_state_lock()?;
        workspace_state_path(&self.workspace_root, CONVERSATION_STATE_FILE_NAME)?;
        let state = load_json_file::<ConversationState>(&self.conversation_path)?;
        self.apply_state(state)?;
        self.persist_state()
    }

    fn apply_state(&mut self, state: ConversationState) -> std::io::Result<()> {
        if state.schema_version > 1 {
            return Err(std::io::Error::other(format!(
                "Unsupported conversation schema {}; original file preserved",
                state.schema_version
            )));
        }
        // Legacy summaries used unstable offsets after front truncation. Keep
        // the raw history and recompute rather than applying a stale summary.
        self.summary_state = state.summary_state.unwrap_or_default();
        self.messages = state.messages;
        self.events = state.events;
        crate::agent::turn::repair_incomplete_tools(&mut self.messages);
        if self
            .summary_state
            .summary
            .as_ref()
            .is_some_and(|s| s.covered_messages > self.messages.len())
        {
            return Err(std::io::Error::other(
                "Summary coverage exceeds conversation history; original state preserved",
            ));
        }
        // State saved before the boundary was recorded cannot say which kept
        // turns came after the summary, so none of their thinking is resent.
        if let Some(summary) = &mut self.summary_state.summary
            && summary.thinking_valid_from.is_none()
        {
            summary.thinking_valid_from = Some(self.messages.len());
        }
        Ok(())
    }

    pub fn clear(&mut self) -> std::io::Result<()> {
        self.ensure_state_lock()?;
        crate::activity_log::rotate(&self.activity_log_path())?;
        let old_messages = std::mem::take(&mut self.messages);
        let old_events = std::mem::take(&mut self.events);
        let old_summary = self.summary_state.clone();
        self.summary_state = ConversationSummaryState::default();
        if let Err(err) = self.persist_state() {
            self.messages = old_messages;
            self.events = old_events;
            self.summary_state = old_summary;
            return Err(err);
        }
        Ok(())
    }

    /// Activity of the most recent saved turns, oldest first.
    pub fn recent_activity(&self) -> Vec<ActivityEvent> {
        crate::activity_log::load_recent(
            &self.activity_log_path(),
            crate::activity_log::REPLAY_TURNS,
        )
    }

    fn activity_log_path(&self) -> PathBuf {
        self.workspace_root
            .join(STATE_DIR_NAME)
            .join(ACTIVITY_LOG_FILE_NAME)
    }

    /// Tolerated configuration problems to show once at startup.
    pub fn startup_warnings(&self) -> Vec<String> {
        let mut warnings = self.warnings.clone();
        warnings.extend(self.provider.startup_warnings());
        warnings
    }

    pub fn transcript_entries(&self) -> Vec<TranscriptEntry> {
        self.messages
            .iter()
            .filter_map(|message| {
                let text = match &message.content {
                    crate::agent::types::MessageContent::Text(text) => text.clone(),
                    crate::agent::types::MessageContent::Blocks(blocks) => blocks
                        .iter()
                        .filter_map(|block| match block {
                            crate::agent::types::ContentBlock::Text { text } => Some(text.clone()),
                            crate::agent::types::ContentBlock::ToolUse { name, input, .. } => {
                                Some(format!("Tool: {name} {input}"))
                            }
                            crate::agent::types::ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => Some(format!(
                                "Tool result {tool_use_id} (error={}): {content}",
                                is_error == &Some(true)
                            )),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                if text.trim().is_empty() {
                    return None;
                }
                let role = match message.role {
                    crate::agent::types::Role::User => TranscriptRole::User,
                    crate::agent::types::Role::Assistant => TranscriptRole::Assistant,
                    crate::agent::types::Role::System => TranscriptRole::System,
                };
                Some(TranscriptEntry { role, text })
            })
            .collect()
    }

    pub async fn submit(&mut self, user_input: String) -> Result<AgentReply, AgentError> {
        self.control.reset_cancellation();
        self.submit_with_progress(user_input, |_| {}).await
    }

    pub async fn submit_with_progress<F>(
        &mut self,
        user_input: String,
        mut on_progress: F,
    ) -> Result<AgentReply, AgentError>
    where
        F: FnMut(AgentProgress) + Send,
    {
        self.ensure_state_lock()?;
        crate::agent::turn::repair_incomplete_tools(&mut self.messages);
        let turn_id = self.messages.len();
        let clock = ActivityClock::start(turn_id);
        let mut recorder = ActivityRecorder::new(self.activity_log_path());
        let mut on_progress = move |event: AgentProgress| {
            let warning = match &event {
                AgentProgress::Activity(activity) => recorder.observe(activity),
                _ => None,
            };
            on_progress(event);
            if let Some(message) = warning {
                on_progress(AgentProgress::Activity(
                    clock.event(0, ActivityKind::Warning { message }),
                ));
            }
        };
        let emit = |on_progress: &mut dyn FnMut(AgentProgress), kind: ActivityKind| {
            on_progress(AgentProgress::Activity(clock.event(0, kind)));
        };
        self.events.push(serde_json::json!({"type":"turn_started","turn_id":turn_id,"sequence":self.events.len()}));
        self.messages.push(Message::user_text(user_input));
        self.persist_state()?;
        emit(&mut on_progress, ActivityKind::TurnStarted);
        if self.should_auto_compact()? {
            on_progress(AgentProgress::CompactionStarted);
            emit(&mut on_progress, ActivityKind::CompactionStarted);
            match self.run_compaction().await {
                Ok(report) => {
                    on_progress(AgentProgress::CompactionFinished {
                        covered_messages: report.covered_messages,
                        used_llm: report.used_llm,
                    });
                    emit(
                        &mut on_progress,
                        ActivityKind::CompactionFinished {
                            covered_messages: report.covered_messages,
                            used_llm: report.used_llm,
                        },
                    );
                }
                // Compaction must never block the conversation; the
                // budget-based trimming below still bounds the prompt.
                Err(AgentError::NothingToCompact) => {}
                Err(err) => eprintln!("Auto-compaction failed: {err}"),
            }
        }
        let prepared_context = prepare_context_messages(
            &self.messages,
            &self.summary_state,
            self.active_model.context_window,
            &self.active_model.id,
            self.max_tokens,
            &self.tools,
        )?;
        let initial_len = prepared_context.messages.len();
        let mut context_messages = prepared_context.messages;
        let turn_context = TurnContext {
            provider: self.provider.as_ref(),
            client: &self.client,
            model: &self.active_model.id,
            context_window: self.active_model.context_window,
            max_tokens: self.max_tokens,
            tools: &self.tools,
            workspace_root: &self.workspace_root,
            control: &self.control,
            clock,
        };
        let raw_len = self.messages.len();
        let active_model = self.selected_model.clone();
        self.summary_state = prepared_context.summary_state;
        let path = &self.conversation_path;
        let raw_messages = &mut self.messages;
        let summary = &self.summary_state;
        let events = &mut self.events;
        let mut checkpoint = |context: &[Message],
                              event: serde_json::Value|
         -> std::io::Result<()> {
            raw_messages.truncate(raw_len);
            raw_messages.extend_from_slice(&context[initial_len..]);
            events.push(serde_json::json!({"sequence":events.len(),"turn_id":turn_id,"message_offset":raw_messages.len(),"event":event}));
            save_json_file(
                path,
                &ConversationStateRef {
                    schema_version: 1,
                    messages: raw_messages,
                    events,
                    summary_state: summary,
                    active_model: active_model.as_deref(),
                },
            )
        };
        let result = run_agent_turn(
            &turn_context,
            &mut context_messages,
            &mut on_progress,
            &mut checkpoint,
        )
        .await;
        self.events.push(serde_json::json!({"type":if result.is_ok(){"turn_finished"}else{"turn_failed"},"sequence":self.events.len(),"turn_id":turn_id,"error":result.as_ref().err().map(ToString::to_string)}));
        on_progress(AgentProgress::Activity(clock.event(
            0,
            ActivityKind::TurnFinished {
                ok: result.is_ok(),
                error: result.as_ref().err().map(ToString::to_string),
            },
        )));
        if let Err(error) = self.persist_state() {
            return Err(std::io::Error::other(format!(
                "Cannot persist turn completion: {error}; previous failure: {:?}",
                result.as_ref().err()
            ))
            .into());
        }
        let outcome = result?;
        if outcome.last_usage.is_some() {
            self.last_measured_usage = outcome.last_usage;
        }
        Ok(AgentReply {
            text: outcome.assistant_text_chunks.join("\n"),
        })
    }

    pub fn context_metrics(&self, pending_user_input: &str) -> std::io::Result<ContextMetrics> {
        let mut metrics = context_metrics_for_input(
            &self.messages,
            pending_user_input,
            &self.summary_state,
            self.active_model.context_window,
            &self.active_model.id,
            self.max_tokens,
            &self.tools,
        )?;
        metrics.apply_measured_input(
            self.last_measured_usage
                .map(|usage| usage.total_input_tokens()),
        );
        Ok(metrics)
    }

    /// Force a compaction regardless of the token threshold (the `/compact`
    /// command). Returns `NothingToCompact` when the whole conversation
    /// already fits in the retained window.
    pub async fn compact(&mut self) -> Result<CompactionReport, AgentError> {
        self.ensure_state_lock()?;
        self.run_compaction().await
    }

    fn should_auto_compact(&self) -> Result<bool, AgentError> {
        let Some(usage) = self.last_measured_usage else {
            return Ok(false);
        };
        let budget = resolve_context_budget(self.active_model.context_window, self.max_tokens)?;
        let threshold = compaction_threshold_tokens(budget.input_budget_tokens());
        Ok(usage.total_input_tokens() >= threshold)
    }

    async fn run_compaction(&mut self) -> Result<CompactionReport, AgentError> {
        let boundary = retention_boundary(&self.messages, COMPACTION_RETAIN_RECENT_MESSAGES);
        let covered = self
            .summary_state
            .summary
            .as_ref()
            .map(|summary| summary.covered_messages)
            .unwrap_or(0);
        if boundary <= covered {
            return Err(AgentError::NothingToCompact);
        }

        let previous_summary = self
            .summary_state
            .summary
            .as_ref()
            .map(|summary| summary.text.clone());
        let compaction_request = CompactionRequest {
            previous_summary: previous_summary.as_deref(),
            messages: &self.messages[covered..boundary],
        };
        let summary_result = tokio::select! {
            biased;
            _ = self.control.cancelled() => return Err(AgentError::Cancelled),
            result = summarize_with_llm(
                self.compaction_provider.as_deref().unwrap_or(self.provider.as_ref()),
                &self.compaction_model,
                &self.client,
                &compaction_request,
            ) => result,
        };
        let (text, used_llm) = match summary_result {
            Ok(text) => (text, true),
            Err(err) => {
                eprintln!("LLM compaction failed, falling back to deterministic summary: {err}");
                let text = match previous_summary.as_deref() {
                    Some(previous) => {
                        extend_summary_text(previous, &self.messages, covered, boundary)
                    }
                    None => summarize_messages(&self.messages[..boundary]),
                };
                (text, false)
            }
        };

        self.summary_state = ConversationSummaryState {
            summary: Some(ConversationSummary {
                covered_messages: boundary,
                text,
                // Everything after the cut so far was produced against the
                // replaced history.
                thinking_valid_from: Some(self.messages.len()),
            }),
        };
        self.persist_state()?;

        // The old measurement described the pre-compaction prompt; drop it
        // so metrics reflect the compacted context until the next call.
        self.last_measured_usage = None;
        let metrics = self.context_metrics("")?;
        Ok(CompactionReport {
            covered_messages: boundary,
            retained_messages: self.messages.len() - boundary,
            used_llm,
            metrics,
        })
    }

    fn persist_state(&self) -> std::io::Result<()> {
        save_json_file(
            &self.conversation_path,
            &ConversationStateRef {
                schema_version: 1,
                messages: &self.messages,
                events: &self.events,
                summary_state: &self.summary_state,
                active_model: self.selected_model.as_deref(),
            },
        )
    }
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod security_tests;

#[cfg(test)]
mod tests {
    use crate::agent::types::{ContentBlock, Message, MessageContent, Role};
    use crate::constants::{CONVERSATION_STATE_FILE_NAME, STATE_DIR_NAME};
    use crate::providers::TokenUsage;
    use crate::providers::anthropic::AnthropicProvider;
    use crate::test_support::{ScriptedProvider, make_temp_dir, text_provider_response};
    use crate::{AgentService, TranscriptEntry, TranscriptRole};
    use serde_json::json;

    #[test]
    fn transcript_entries_include_text_and_tool_results() {
        let workspace = make_temp_dir("transcript");
        let conversation_path = workspace
            .path()
            .join(STATE_DIR_NAME)
            .join(CONVERSATION_STATE_FILE_NAME);
        let messages = vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("hello".to_string()),
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text {
                        text: "answer".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool_1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "src/lib.rs"}),
                    },
                ]),
            },
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "tool_1".to_string(),
                    content: "Path: src/lib.rs".to_string(),
                    is_error: None,
                }]),
            },
        ];

        let mut service = AgentService::with_provider(
            workspace.path().to_path_buf(),
            Box::new(AnthropicProvider::with_credentials("test", "2023-06-01")),
            "test-model".to_string(),
            128,
            reqwest::Client::builder().build().unwrap(),
        );
        service.conversation_path = conversation_path;
        service.messages = messages;

        let entries = service.transcript_entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[0],
            TranscriptEntry {
                role: TranscriptRole::User,
                text: "hello".into()
            }
        );
        assert!(entries[1].text.contains("Tool: read_file"));
        assert!(entries[2].text.contains("Tool result tool_1"));
    }

    fn service_with_scripted_provider(
        workspace: &std::path::Path,
        provider: ScriptedProvider,
        messages: Vec<Message>,
        last_measured_usage: Option<TokenUsage>,
    ) -> AgentService {
        let mut service = AgentService::with_provider(
            workspace.to_path_buf(),
            Box::new(provider),
            "claude-sonnet-4-5".to_string(),
            1_000,
            reqwest::Client::builder().build().unwrap(),
        );
        service.messages = messages;
        service.last_measured_usage = last_measured_usage;
        service
    }

    fn plain_user_messages(count: usize) -> Vec<Message> {
        (0..count)
            .map(|index| Message::user_text(format!("message {index}")))
            .collect()
    }

    #[tokio::test]
    async fn submit_auto_compacts_when_measured_usage_exceeds_threshold() {
        let workspace = make_temp_dir("auto_compact");
        let over_threshold = TokenUsage {
            input_tokens: 170_000,
            ..TokenUsage::default()
        };
        let provider = ScriptedProvider::new(vec![
            Ok(text_provider_response("1. User request: summary", None)),
            Ok(text_provider_response("turn answer", None)),
        ]);
        let mut service = service_with_scripted_provider(
            workspace.path(),
            provider,
            plain_user_messages(30),
            Some(over_threshold),
        );

        let mut progress_events = Vec::new();
        let reply = service
            .submit_with_progress("next question".to_string(), |event| {
                progress_events.push(event)
            })
            .await
            .unwrap();
        assert_eq!(reply.text, "turn answer");

        let summary = service.summary_state.summary.as_ref().unwrap();
        // 31 messages at compaction time, 16 retained.
        assert_eq!(summary.covered_messages, 15);
        assert_eq!(summary.text, "1. User request: summary");
        assert!(
            progress_events.contains(&crate::AgentProgress::CompactionFinished {
                covered_messages: 15,
                used_llm: true,
            })
        );

        let persisted: crate::agent::state::ConversationState =
            crate::agent::state::load_json_file(
                &workspace
                    .path()
                    .join(STATE_DIR_NAME)
                    .join(CONVERSATION_STATE_FILE_NAME),
            )
            .unwrap();
        assert_eq!(
            persisted
                .summary_state
                .unwrap()
                .summary
                .unwrap()
                .covered_messages,
            15
        );
    }

    #[tokio::test]
    async fn auto_compaction_threshold_leaves_room_for_the_output_allowance() {
        let workspace = make_temp_dir("auto_compact_output_room");
        // 200k window - 64k output = 136k input budget; 80% of that is
        // 108.8k. This prompt fits the budget now but not for long.
        let near_budget = TokenUsage {
            input_tokens: 120_000,
            ..TokenUsage::default()
        };
        let provider = ScriptedProvider::new(vec![
            Ok(text_provider_response("1. User request: summary", None)),
            Ok(text_provider_response("turn answer", None)),
        ]);
        let mut service = service_with_scripted_provider(
            workspace.path(),
            provider,
            plain_user_messages(30),
            Some(near_budget),
        );
        service.max_tokens = 64_000;

        service.submit("next".to_string()).await.unwrap();

        assert!(service.summary_state.summary.is_some());
    }

    #[tokio::test]
    async fn submit_skips_compaction_below_threshold() {
        let workspace = make_temp_dir("no_auto_compact");
        let below_threshold = TokenUsage {
            input_tokens: 10_000,
            ..TokenUsage::default()
        };
        let provider = ScriptedProvider::new(vec![Ok(text_provider_response("turn answer", None))]);
        let mut service = service_with_scripted_provider(
            workspace.path(),
            provider,
            plain_user_messages(30),
            Some(below_threshold),
        );

        let mut progress_events = Vec::new();
        service
            .submit_with_progress("next question".to_string(), |event| {
                progress_events.push(event)
            })
            .await
            .unwrap();

        // Exactly one model call (the turn itself): no compaction request
        // was issued and no compaction progress was reported.
        assert!(
            !progress_events
                .iter()
                .any(|event| matches!(event, crate::AgentProgress::CompactionStarted))
        );
    }

    #[tokio::test]
    async fn compact_falls_back_to_deterministic_summary_on_llm_error() {
        let workspace = make_temp_dir("compact_fallback");
        let provider = ScriptedProvider::new(vec![Err(
            crate::providers::error::ProviderError::Api("model unavailable".to_string()),
        )]);
        let mut service = service_with_scripted_provider(
            workspace.path(),
            provider,
            plain_user_messages(30),
            None,
        );

        let report = service.compact().await.unwrap();

        assert!(!report.used_llm);
        assert_eq!(report.covered_messages, 14);
        assert_eq!(report.retained_messages, 16);
        let summary = service.summary_state.summary.unwrap();
        assert!(summary.text.contains("message 0"));
    }

    #[tokio::test]
    async fn compact_reports_nothing_to_do_for_short_conversations() {
        let workspace = make_temp_dir("compact_short");
        let provider = ScriptedProvider::new(vec![]);
        let mut service = service_with_scripted_provider(
            workspace.path(),
            provider,
            plain_user_messages(5),
            None,
        );

        let result = service.compact().await;

        assert!(matches!(result, Err(crate::AgentError::NothingToCompact)));
    }

    mod thinking_after_compaction {
        use super::*;
        use crate::agent::state::{ConversationState, load_json_file};
        use crate::agent::types::AgentMessageResponse;
        use crate::providers::ProviderResponse;
        use crate::providers::error::ProviderError;

        /// Each turn: a request, a tool call with thinking, its result, and
        /// an answer with thinking. Four messages per turn.
        fn history(turns: usize) -> Vec<Message> {
            (0..turns)
                .flat_map(|turn| {
                    [
                        Message::user_text(format!("request {turn}")),
                        Message::assistant_blocks(vec![
                            ContentBlock::Thinking {
                                thinking: format!("plan {turn}"),
                                signature: format!("sig-call-{turn}"),
                            },
                            ContentBlock::RedactedThinking {
                                data: format!("redacted-{turn}"),
                            },
                            ContentBlock::ToolUse {
                                id: format!("tool_{turn}"),
                                name: "read_file".into(),
                                input: json!({"path": "missing.txt"}),
                            },
                        ]),
                        Message::user_blocks(vec![ContentBlock::ToolResult {
                            tool_use_id: format!("tool_{turn}"),
                            content: "not found".into(),
                            is_error: Some(true),
                        }]),
                        Message::assistant_blocks(vec![
                            ContentBlock::Thinking {
                                thinking: String::new(),
                                signature: format!("sig-answer-{turn}"),
                            },
                            ContentBlock::Text {
                                text: format!("done {turn}"),
                            },
                        ]),
                    ]
                })
                .collect()
        }

        fn response(blocks: Vec<ContentBlock>) -> Result<ProviderResponse, ProviderError> {
            Ok(ProviderResponse {
                message: AgentMessageResponse { content: blocks },
                streamed_text: false,
                usage: None,
            })
        }

        fn thinking_then_tool(signature: &str, id: &str) -> Vec<ContentBlock> {
            vec![
                ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: signature.into(),
                },
                ContentBlock::ToolUse {
                    id: id.into(),
                    name: "read_file".into(),
                    input: json!({"path": "missing.txt"}),
                },
            ]
        }

        fn thinking_then_text(signature: &str) -> Vec<ContentBlock> {
            vec![
                ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: signature.into(),
                },
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ]
        }

        /// Signatures and redacted data the request sends back.
        fn sent_thinking(request: &serde_json::Value) -> Vec<String> {
            let mut found = Vec::new();
            for message in request["messages"].as_array().unwrap() {
                for block in message["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("thinking") => found.push(block["signature"].as_str().unwrap().into()),
                        Some("redacted_thinking") => {
                            found.push(block["data"].as_str().unwrap().into())
                        }
                        _ => {}
                    }
                }
            }
            found
        }

        /// Every tool_result answers a tool_use in the assistant message
        /// right before it, and every tool_use is answered.
        fn assert_tool_pairs(request: &serde_json::Value) {
            let messages = request["messages"].as_array().unwrap();
            for (index, message) in messages.iter().enumerate() {
                let blocks = message["content"].as_array().into_iter().flatten();
                for block in blocks {
                    if block["type"] == "tool_use" {
                        let next = &messages[index + 1]["content"];
                        assert!(
                            next.as_array().unwrap().iter().any(|result| {
                                result["type"] == "tool_result"
                                    && result["tool_use_id"] == block["id"]
                            }),
                            "unanswered {block}"
                        );
                    }
                    if block["type"] == "tool_result" {
                        let previous = &messages[index - 1]["content"];
                        assert!(
                            previous.as_array().unwrap().iter().any(|call| {
                                call["type"] == "tool_use" && call["id"] == block["tool_use_id"]
                            }),
                            "orphaned {block}"
                        );
                    }
                }
            }
        }

        type Requests = std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>;

        fn scripted(
            workspace: &std::path::Path,
            responses: Vec<Result<ProviderResponse, ProviderError>>,
            messages: Vec<Message>,
            usage: Option<TokenUsage>,
        ) -> (AgentService, Requests) {
            let provider = ScriptedProvider::new(responses);
            let requests = provider.recorded_requests.clone();
            (
                service_with_scripted_provider(workspace, provider, messages, usage),
                requests,
            )
        }

        #[tokio::test]
        async fn explicit_compaction_resends_only_thinking_produced_after_the_cut() {
            let workspace = make_temp_dir("thinking_explicit");
            let (mut service, requests) = scripted(
                workspace.path(),
                vec![
                    Ok(text_provider_response("1. User request: summary", None)),
                    response(thinking_then_tool("sig-new-call", "tool_new")),
                    response(thinking_then_text("sig-new-answer")),
                    response(thinking_then_text("sig-later")),
                ],
                history(8),
                None,
            );

            let report = service.compact().await.unwrap();
            assert_eq!(report.covered_messages, 16);
            service.submit("next".into()).await.unwrap();

            // The first request after the cut: every kept turn was produced
            // against the replaced history.
            let first = requests.lock().unwrap()[1].clone();
            assert!(
                sent_thinking(&first).is_empty(),
                "{:?}",
                sent_thinking(&first)
            );
            assert_tool_pairs(&first);
            assert!(first.to_string().contains("tool_7"));
            // Within the same turn, the open tool call keeps its thinking.
            let second = requests.lock().unwrap()[2].clone();
            assert_eq!(sent_thinking(&second), ["sig-new-call"]);
            assert_tool_pairs(&second);

            service.submit("again".into()).await.unwrap();
            let later = requests.lock().unwrap()[3].clone();
            assert_eq!(sent_thinking(&later), ["sig-new-call", "sig-new-answer"]);
            assert_tool_pairs(&later);

            // The raw history keeps every block for the record.
            let saved = serde_json::to_string(&service.messages).unwrap();
            assert!(saved.contains("sig-call-5") && saved.contains("redacted-5"));
        }

        #[tokio::test]
        async fn auto_compaction_drops_kept_thinking_from_the_turn_that_triggered_it() {
            let workspace = make_temp_dir("thinking_auto");
            let (mut service, requests) = scripted(
                workspace.path(),
                vec![
                    Ok(text_provider_response("1. User request: summary", None)),
                    response(thinking_then_text("sig-new")),
                ],
                history(8),
                Some(TokenUsage {
                    input_tokens: 170_000,
                    ..TokenUsage::default()
                }),
            );

            service.submit("next".into()).await.unwrap();

            assert!(service.summary_state.summary.is_some());
            let turn = requests.lock().unwrap()[1].clone();
            assert!(
                sent_thinking(&turn).is_empty(),
                "{:?}",
                sent_thinking(&turn)
            );
            assert_tool_pairs(&turn);
        }

        #[tokio::test]
        async fn redacted_thinking_goes_back_unchanged_with_tool_results_and_after_restart() {
            let data = "EmwKAhgBEgy3va3pzix/LafPsn4aDFIT2Xlxh0L5L8rLVyIwxtE3rAFBa8cr3qpP+/=";
            let workspace = make_temp_dir("redacted_round_trip");
            let (mut service, requests) = scripted(
                workspace.path(),
                vec![
                    response(vec![
                        ContentBlock::RedactedThinking { data: data.into() },
                        ContentBlock::ToolUse {
                            id: "tool_r".into(),
                            name: "read_file".into(),
                            input: json!({"path": "missing.txt"}),
                        },
                    ]),
                    response(thinking_then_text("sig-final")),
                ],
                Vec::new(),
                None,
            );

            service.submit("look".into()).await.unwrap();

            // The tool round sends the turn back exactly: redacted block
            // first, then the call it preceded, then the result.
            let round = requests.lock().unwrap()[1].clone();
            let messages = round["messages"].as_array().unwrap();
            assert_eq!(
                messages[1]["content"][0],
                json!({"type": "redacted_thinking", "data": data})
            );
            assert_eq!(messages[1]["content"][1]["id"], "tool_r");
            assert_eq!(messages[2]["content"][0]["tool_use_id"], "tool_r");
            // Nothing of it is shown as text.
            let shown = format!("{:?}", service.transcript_entries());
            assert!(!shown.contains(data), "{shown}");
            drop(service);

            let (mut restarted, requests) = scripted(
                workspace.path(),
                vec![response(thinking_then_text("sig-next"))],
                Vec::new(),
                None,
            );
            restarted.restore_state().unwrap();
            restarted.submit("again".into()).await.unwrap();
            let request = requests.lock().unwrap()[0].clone();
            assert_eq!(sent_thinking(&request), [data, "sig-final"]);
            assert_eq!(
                request["messages"][1]["content"][0],
                json!({"type": "redacted_thinking", "data": data})
            );
            assert_tool_pairs(&request);
        }

        #[tokio::test]
        async fn restart_keeps_the_boundary_and_treats_legacy_summaries_as_stale() {
            let workspace = make_temp_dir("thinking_restart");
            let (mut service, _) = scripted(
                workspace.path(),
                vec![
                    Ok(text_provider_response("1. User request: summary", None)),
                    response(thinking_then_text("sig-new")),
                ],
                history(8),
                None,
            );
            service.compact().await.unwrap();
            service.submit("next".into()).await.unwrap();
            let path = service.conversation_path.clone();
            drop(service);

            let saved: ConversationState = load_json_file(&path).unwrap();
            let boundary = saved.summary_state.unwrap().summary.unwrap();
            assert_eq!(boundary.thinking_valid_from, Some(32));

            let (mut restarted, requests) = scripted(
                workspace.path(),
                vec![response(thinking_then_text("sig-after"))],
                Vec::new(),
                None,
            );
            restarted.restore_state().unwrap();
            restarted.submit("after restart".into()).await.unwrap();
            assert_eq!(sent_thinking(&requests.lock().unwrap()[0]), ["sig-new"]);
            drop(restarted);

            // A summary saved before the boundary existed: no kept turn may
            // be assumed to follow it.
            let mut legacy: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            legacy["summary_state"]["summary"]
                .as_object_mut()
                .unwrap()
                .remove("thinking_valid_from");
            std::fs::write(&path, legacy.to_string()).unwrap();
            let (mut restarted, requests) = scripted(
                workspace.path(),
                vec![response(thinking_then_text("sig-x"))],
                Vec::new(),
                None,
            );
            restarted.restore_state().unwrap();
            let length = restarted.messages.len();
            let summary = restarted.summary_state.summary.as_ref().unwrap();
            assert_eq!(summary.thinking_valid_from, Some(length));
            restarted.submit("legacy".into()).await.unwrap();
            let request = requests.lock().unwrap()[0].clone();
            assert!(sent_thinking(&request).is_empty());
            assert_tool_pairs(&request);
        }
    }

    #[tokio::test]
    async fn submit_records_measured_usage_and_exposes_it_in_metrics() {
        let workspace = make_temp_dir("measured_usage");
        let usage = TokenUsage {
            input_tokens: 1_000,
            output_tokens: 50,
            cache_creation_input_tokens: 200,
            cache_read_input_tokens: 300,
        };
        let provider =
            ScriptedProvider::new(vec![Ok(text_provider_response("hi there", Some(usage)))]);
        let mut service = AgentService::with_provider(
            workspace.path().to_path_buf(),
            Box::new(provider),
            "claude-sonnet-4-5".to_string(),
            1_000,
            reqwest::Client::builder().build().unwrap(),
        );

        let metrics_before = service.context_metrics("").unwrap();
        assert_eq!(metrics_before.measured_input_tokens, None);

        let reply = service.submit("hello".to_string()).await.unwrap();
        assert_eq!(reply.text, "hi there");

        let metrics_after = service.context_metrics("").unwrap();
        assert_eq!(metrics_after.measured_input_tokens, Some(1_500));
    }

    mod model_switching {
        use super::*;
        use crate::models::ModelRegistry;
        use crate::tools::ToolDefinition;
        use std::collections::HashMap;
        use std::sync::Arc;

        const MODELS: &str = r#"
[providers.deepseek]
wire = "anthropic"
base_url = "https://api.deepseek.com/anthropic"
api_key_env = "DEEPSEEK_API_KEY"
auth = "bearer"

[[models]]
alias = "ds-flash"
provider = "deepseek"
id = "deepseek-flash"
context_window = 1000000
max_output_tokens = 32000
"#;

        fn lookup(pairs: &[(&str, &str)]) -> super::super::EnvLookup {
            let map = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>();
            Arc::new(move |key| map.get(key).cloned())
        }

        fn open(root: &std::path::Path, env: &[(&str, &str)]) -> AgentService {
            AgentService::open(
                root.to_path_buf(),
                ModelRegistry::from_toml(MODELS).unwrap(),
                lookup(env),
                None,
                reqwest::Client::new(),
            )
            .unwrap()
        }

        const KEYS: [(&str, &str); 2] = [("ANTHROPIC_API_KEY", "a"), ("DEEPSEEK_API_KEY", "d")];

        fn with_model(model: &str) -> Vec<(&str, &str)> {
            let mut env = KEYS.to_vec();
            env.push(("MODEL", model));
            env
        }

        fn saved_state(root: &std::path::Path) -> serde_json::Value {
            let path = root.join(STATE_DIR_NAME).join(CONVERSATION_STATE_FILE_NAME);
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
        }

        fn thinking_turn() -> Vec<Message> {
            vec![
                Message::user_text("hi".into()),
                Message::assistant_blocks(vec![
                    ContentBlock::Thinking {
                        thinking: "plan".into(),
                        signature: "sig".into(),
                    },
                    ContentBlock::Text {
                        text: "answer".into(),
                    },
                ]),
            ]
        }

        fn has_thinking(messages: &[Message]) -> bool {
            messages.iter().any(|message| {
                matches!(&message.content, MessageContent::Blocks(blocks)
                    if blocks.iter().any(|b| matches!(b, ContentBlock::Thinking { .. })))
            })
        }

        #[test]
        fn startup_uses_model_env_then_the_registry_default() {
            let root = make_temp_dir("model_startup");
            let service = open(root.path(), &with_model("sonnet"));
            assert_eq!(service.active_model().alias, "sonnet");
            assert!(saved_state(root.path()).get("active_model").is_none());
            drop(service);

            // Only a /model choice is saved, so a changed MODEL takes effect.
            let service = open(root.path(), &KEYS);
            assert_eq!(service.active_model().alias, "opus");
            assert_eq!(service.compaction_model, "claude-haiku-5-5");
            assert!(service.compaction_provider.is_some());
        }

        #[test]
        fn a_switch_is_saved_and_wins_over_model_env_after_restart() {
            let root = make_temp_dir("model_saved");
            let mut service = open(root.path(), &with_model("sonnet"));
            service.messages = thinking_turn();
            let switched = service.switch_model("ds-flash").unwrap();
            assert_eq!(switched.model.model_id, "deepseek-flash");
            assert!(switched.model.active);
            assert!(!has_thinking(&service.messages));
            let state = saved_state(root.path());
            assert_eq!(state["active_model"], "ds-flash");
            assert!(!state.to_string().contains("\"thinking\""));
            drop(service);

            let service = open(root.path(), &with_model("sonnet"));
            assert_eq!(service.active_model().alias, "ds-flash");
            assert!(service.startup_warnings().is_empty());
        }

        #[test]
        fn an_unavailable_saved_model_falls_back_with_a_warning_and_strips_thinking() {
            let root = make_temp_dir("model_fallback");
            let mut service = open(root.path(), &KEYS);
            service.switch_model("ds-flash").unwrap();
            service.messages = thinking_turn();
            service.persist_state().unwrap();
            drop(service);

            let service = open(
                root.path(),
                &[("ANTHROPIC_API_KEY", "a"), ("MODEL", "sonnet")],
            );
            assert_eq!(service.active_model().alias, "sonnet");
            let warnings = service.startup_warnings();
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].contains("'ds-flash'"));
            assert!(warnings[0].contains("DEEPSEEK_API_KEY is not set"));
            assert!(!has_thinking(&service.messages));
            assert!(saved_state(root.path()).get("active_model").is_none());
        }

        #[test]
        fn switching_to_a_smaller_window_asks_for_compaction() {
            let root = make_temp_dir("model_small_window");
            let registry = ModelRegistry::from_toml(
                r#"
[[models]]
alias = "small"
provider = "anthropic"
id = "claude-small"
context_window = 20000
max_output_tokens = 4000
"#,
            )
            .unwrap();
            let mut service = AgentService::open(
                root.path().to_path_buf(),
                registry,
                lookup(&KEYS),
                None,
                reqwest::Client::new(),
            )
            .unwrap();
            service.messages = (0..40)
                .map(|i| Message::user_text(format!("{i} {}", "word ".repeat(400))))
                .collect();

            let small = service.switch_model("small").unwrap();
            assert!(small.needs_compaction);
            assert!(
                small
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("small's input budget of 16000"))
            );

            let large = service.switch_model("opus").unwrap();
            assert!(!large.needs_compaction);
            assert!(large.warnings.is_empty(), "{:?}", large.warnings);
        }

        #[test]
        fn choosing_the_current_model_saves_the_choice() {
            let root = make_temp_dir("model_same");
            let mut service = open(root.path(), &with_model("sonnet"));
            service.messages = thinking_turn();
            service.switch_model("sonnet").unwrap();
            assert!(has_thinking(&service.messages));
            assert_eq!(saved_state(root.path())["active_model"], "sonnet");
        }

        #[test]
        fn ignored_settings_are_warned_about_once() {
            let root = make_temp_dir("model_warn_once");
            let mut env = KEYS.to_vec();
            env.extend([("EFFORT", "high"), ("MODEL", "ds-flash")]);
            let mut service = open(root.path(), &env);
            let warnings = service.startup_warnings();
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].contains("EFFORT is ignored for ds-flash"));

            service.switch_model("opus").unwrap();
            let back = service.switch_model("ds-flash").unwrap();
            assert_eq!(back.warnings.len(), 1, "{:?}", back.warnings);
        }

        #[test]
        fn a_saved_alias_removed_from_the_registry_is_reported() {
            let root = make_temp_dir("model_removed");
            let mut service = open(root.path(), &KEYS);
            service.switch_model("ds-flash").unwrap();
            drop(service);

            let service = AgentService::open(
                root.path().to_path_buf(),
                ModelRegistry::builtin(),
                lookup(&KEYS),
                None,
                reqwest::Client::new(),
            )
            .unwrap();
            assert_eq!(service.active_model().alias, "opus");
            assert!(service.startup_warnings()[0].contains("'ds-flash' is no longer registered"));
        }

        #[test]
        fn without_the_default_key_startup_uses_a_model_that_has_one() {
            let root = make_temp_dir("model_other_key");
            let service = open(root.path(), &[("DEEPSEEK_API_KEY", "d")]);
            assert_eq!(service.active_model().alias, "ds-flash");
            let warnings = service.startup_warnings();
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].contains("'opus' is unavailable: ANTHROPIC_API_KEY is not set"));
            assert!(warnings[0].contains("started with 'ds-flash'"));
            assert!(saved_state(root.path()).get("active_model").is_none());
        }

        #[test]
        fn the_last_candidate_failing_is_a_startup_error() {
            let root = make_temp_dir("model_no_key");
            let err = AgentService::open(
                root.path().to_path_buf(),
                ModelRegistry::builtin(),
                lookup(&[]),
                None,
                reqwest::Client::new(),
            )
            .err()
            .unwrap();
            assert!(err.to_string().contains("ANTHROPIC_API_KEY is not set"));
        }

        #[test]
        fn switching_applies_the_models_limits_tools_and_compaction() {
            let root = make_temp_dir("model_limits");
            let mut service = open(root.path(), &KEYS);
            assert!(
                service
                    .tools
                    .iter()
                    .any(|tool| matches!(tool, ToolDefinition::AnthropicTextEditor(_)))
            );
            service.last_measured_usage = Some(TokenUsage::default());

            service.switch_model("ds-flash").unwrap();
            assert_eq!(service.max_tokens, 32_000);
            assert!(
                service
                    .tools
                    .iter()
                    .all(|tool| matches!(tool, ToolDefinition::Custom(_)))
            );
            assert_eq!(service.compaction_model, "deepseek-flash");
            assert!(service.compaction_provider.is_none());
            assert!(service.last_measured_usage.is_none());
            assert_eq!(
                service.context_metrics("").unwrap().context_window_tokens,
                1_000_000
            );
        }

        #[test]
        fn unknown_or_unavailable_models_leave_the_current_one() {
            let root = make_temp_dir("model_reject");
            let mut service = open(root.path(), &[("ANTHROPIC_API_KEY", "a")]);
            service.messages = thinking_turn();

            let unknown = service.switch_model("gpt-9").err().unwrap();
            assert!(unknown.to_string().contains("Unknown model 'gpt-9'"));
            let missing = service.switch_model("ds-flash").err().unwrap();
            assert!(missing.to_string().contains("DEEPSEEK_API_KEY is not set"));

            assert_eq!(service.active_model().alias, "opus");
            assert!(has_thinking(&service.messages));
        }

        #[test]
        fn the_model_list_marks_the_active_model_and_missing_keys() {
            let root = make_temp_dir("model_list");
            let service = open(root.path(), &[("ANTHROPIC_API_KEY", "a")]);
            let choices = service.available_models();
            let aliases = choices.iter().map(|c| c.alias.as_str()).collect::<Vec<_>>();
            assert_eq!(aliases, ["opus", "sonnet", "haiku", "ds-flash"]);
            assert!(choices[0].active && choices[0].unavailable.is_none());
            assert_eq!(
                choices[3].unavailable.as_deref(),
                Some("DEEPSEEK_API_KEY is not set")
            );
        }

        #[test]
        fn an_ad_hoc_model_env_is_listed_but_not_saved() {
            let root = make_temp_dir("model_ad_hoc");
            let service = open(root.path(), &with_model("claude-future-1"));
            assert_eq!(service.active_model().model_id, "claude-future-1");
            assert!(
                service
                    .available_models()
                    .last()
                    .is_some_and(|choice| choice.active && choice.alias == "claude-future-1")
            );
            assert!(saved_state(root.path()).get("active_model").is_none());
        }

        #[test]
        fn compact_model_env_is_reported_as_unsupported() {
            let root = make_temp_dir("model_compact_env");
            let mut env = KEYS.to_vec();
            env.push(("COMPACT_MODEL", "claude-haiku-5-5"));
            let service = open(root.path(), &env);
            assert!(service.startup_warnings()[0].contains("COMPACT_MODEL is no longer supported"));
        }
    }
}

#[cfg(test)]
mod live_tests;

#[cfg(test)]
mod reliability_tests;
