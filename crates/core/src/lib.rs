pub use crate::agent::state::{
    load_json_file as load_local_state, save_json_file as save_local_state, workspace_state_path,
};
pub mod activity;
mod activity_log;
mod approval;
mod control;
mod execution;
mod instructions;
mod models;
pub mod plan;
mod process;
mod session;
mod settings;
mod transcript;
mod usage;
pub use approval::Grant;
pub use control::{ApprovalAnswer, TaskControl};
pub use execution::SANDBOX_DESCRIPTION;
pub use session::{SessionStart, SessionSummary, list_sessions};
pub use settings::Settings;
pub use transcript::{TranscriptEntry, TranscriptRole};
pub use usage::SessionUsage;
pub mod agent;
pub mod constants;
pub mod error;
pub mod providers;
pub mod tools;

pub use crate::activity::{ActivityEvent, ActivityKind, ThoughtRole};
pub use crate::agent::context::ContextMetrics;
pub use crate::agent::subagent::record::{SubagentStatus, SubagentSummary};
pub use crate::error::AgentError;
pub use crate::plan::PlanView;

use crate::activity::ActivityClock;
use crate::activity_log::{ACTIVITY_LOG_FILE_NAME, ActivityRecorder};
use crate::agent::compaction::{compact_history, compaction_threshold_tokens};
use crate::agent::context::{
    context_metrics_for_input, history_input_tokens, prepare_context_messages,
    resolve_context_budget,
};
use crate::agent::state::{
    ConversationState, ConversationStateRef, ConversationSummaryState, load_json_file,
};
use crate::agent::subagent::catalog::{
    SubagentCatalog, SubagentModel, SubagentType, load_user_types,
};
use crate::agent::turn::{TurnContext, run_agent_turn};
use crate::agent::types::{Message, MessageContent, Role};
use crate::constants::*;
use crate::models::{ModelConfig, ModelRegistry};
use crate::providers::error::ProviderError;
use crate::providers::{Provider, TokenUsage, build_provider};
use crate::tools::ToolDefinition;
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

/// A prompt of this conversation that `/rewind` can go back to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewindPoint {
    /// Where the prompt sits in the history; turns started from here are
    /// dropped.
    pub turn_id: usize,
    pub prompt: String,
    /// The files Socium's edits changed from this prompt on, sorted.
    pub changed_files: Vec<String>,
}

/// What a rewind takes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewindMode {
    ConversationAndFiles,
    Conversation,
    Files,
}

/// Result of a rewind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewound {
    /// The prompt that was taken back, to edit and send again.
    pub prompt: String,
    /// Why the Activity of the dropped turns could not be removed, or
    /// their edits kept out of later rewinds. The rewind is done either
    /// way.
    pub warning: Option<String>,
    /// The files put back to their content before the prompt, sorted.
    pub restored_files: Vec<String>,
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
        if !model.tools {
            warnings.push(format!(
                "'{}' is set to take no tools: it answers in one reply and cannot read, edit, or run anything",
                model.alias
            ));
        }
        let compact = model
            .compaction_alias()
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
    /// What the user should know about how the turn ended, such as an
    /// answer cut at the output limit.
    pub notices: Vec<String>,
}

/// Outcome of a compaction run: how much history the summary now covers
/// and whether the LLM summarizer succeeded or the deterministic fallback
/// was used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionReport {
    pub covered_messages: usize,
    pub retained_messages: usize,
    pub used_llm: bool,
    /// Why the compaction model was not used, when it failed.
    pub fallback_reason: Option<String>,
    pub metrics: ContextMetrics,
    /// The activity event for it, which carries the summary call's usage.
    pub(crate) finished: ActivityKind,
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
        /// The subagent that asked, such as `worker subagent "Fix the
        /// test"`; `None` when the main agent asked.
        requested_by: Option<String>,
        /// The "allow for this session" choice to offer, when there is one.
        session_option: Option<String>,
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
    /// A finished tool call as the transcript shows it after a restart.
    ToolCallRecorded(TranscriptEntry),
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
        /// Whether either stream was cut in the result; the saved
        /// artifacts hold what was left out.
        truncated: bool,
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
    state_lock: Option<crate::agent::state::WorkspaceLock>,
    workspace_root: PathBuf,
    /// The conversation saved under `.socium/sessions/<id>/`.
    session_id: String,
    /// Name given with `/rename`.
    session_name: Option<String>,
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
    /// `CONTEXT_WINDOW_TOKENS`, which replaces every model's window.
    context_window_override: Option<u32>,
    /// `COMPACTION_TOKEN_THRESHOLD`, which replaces every model's threshold.
    compaction_threshold_override: Option<u32>,
    max_tokens: u32,
    tools: Vec<ToolDefinition>,
    /// Types the `agent` tool can start, user types loaded at startup.
    subagents: SubagentCatalog,
    /// The user's sandbox and approval settings from `config.toml`.
    settings: Arc<Settings>,
    client: HttpClient,
    /// Configuration problems found while choosing the startup model.
    warnings: Vec<String>,
    /// Usage measured on the most recent model call. Volatile: not
    /// persisted, so the first turn after a restart falls back to the
    /// heuristic estimate.
    last_measured_usage: Option<TokenUsage>,
    /// How far the current model's measured prompts are from the estimate;
    /// see `ContextBudget::estimate_scale`. Reset when the model changes.
    estimate_scale: f32,
    /// Tokens used by this conversation since it was opened in this process.
    session_usage: Arc<std::sync::Mutex<SessionUsage>>,
    /// Socium's instructions plus the sandbox settings and the project's
    /// instruction file, read when the service is opened.
    system_prompt: String,
    /// The framed instruction file alone, which subagents also get.
    project_instructions: Option<String>,
    /// DeepSeek search, when the user turned `web_search` on and its key is
    /// set.
    web_search: Option<crate::tools::web_search::WebSearch>,
    /// Why the instruction file could not be used when it was last read,
    /// so the same problem is reported once.
    instructions_problem: Option<String>,
    sandbox: SandboxCheck,
}

/// Whether the OS sandbox enforces isolation. The check takes most of the
/// startup time, so it runs on a thread while the interface opens, and no
/// turn starts until it has passed.
enum SandboxCheck {
    Running(std::thread::JoinHandle<Result<(), String>>),
    Passed,
    Failed(String),
}

impl SandboxCheck {
    fn start(workspace_root: PathBuf) -> Self {
        Self::Running(std::thread::spawn(move || probe_sandbox(&workspace_root)))
    }

    /// Records the result of a finished check, waiting for it when `wait`.
    fn settle(&mut self, wait: bool) {
        if let Self::Running(handle) = self
            && (wait || handle.is_finished())
            && let Self::Running(handle) = std::mem::replace(self, Self::Passed)
        {
            let result = handle.join().unwrap_or_else(|_| {
                Err("OS sandbox initialization failed: the check panicked".into())
            });
            if let Err(message) = result {
                *self = Self::Failed(message);
            }
        }
    }
}

/// Runs `true` in the sandbox. A missing backend or disabled namespaces is
/// an error, never a downgrade.
fn probe_sandbox(workspace_root: &std::path::Path) -> Result<(), String> {
    let probe = crate::process::run(
        &["true".into()],
        workspace_root,
        Duration::from_secs(5),
        &TaskControl::default(),
    )
    .map_err(|e| format!("OS sandbox initialization failed: {e}"))?;
    if !probe.status.success() || probe.timed_out {
        return Err(format!(
            "OS sandbox initialization failed; no host fallback: {}",
            String::from_utf8_lossy(&probe.stderr)
        ));
    }
    Ok(())
}

/// The client for model requests. There is no deadline for a whole
/// response: a long answer streams for minutes. Instead the connection must
/// open within 30 seconds and data must keep arriving (SSE pings count).
fn provider_http_client() -> Result<reqwest::Client, ProviderError> {
    http_client(
        Duration::from_secs(30),
        crate::providers::error::IDLE_TIMEOUT,
    )
}

/// Loading the system's CA certificates takes a few milliseconds, so startup
/// builds the client on a thread and the first request waits for it.
pub(crate) enum HttpClient {
    Building(std::thread::JoinHandle<Result<reqwest::Client, ProviderError>>),
    Ready(reqwest::Client),
    Failed(String),
}

impl From<reqwest::Client> for HttpClient {
    fn from(client: reqwest::Client) -> Self {
        Self::Ready(client)
    }
}

impl HttpClient {
    fn start() -> Self {
        Self::Building(std::thread::spawn(provider_http_client))
    }

    fn get(&mut self) -> Result<reqwest::Client, ProviderError> {
        if matches!(self, Self::Building(_))
            && let Self::Building(handle) = std::mem::replace(self, Self::Failed(String::new()))
        {
            *self = match handle.join() {
                Ok(Ok(client)) => Self::Ready(client),
                Ok(Err(err)) => Self::Failed(err.to_string()),
                Err(_) => Self::Failed("the HTTP client could not be built".into()),
            };
        }
        match self {
            Self::Ready(client) => Ok(client.clone()),
            Self::Failed(message) => Err(ProviderError::Env(Error::other(message.clone()))),
            Self::Building(_) => unreachable!("settled above"),
        }
    }
}

fn http_client(connect: Duration, idle: Duration) -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(connect)
        .read_timeout(idle)
        .build()
        .map_err(ProviderError::Http)
}

impl AgentService {
    pub fn from_current_dir(start: SessionStart) -> Result<Self, AgentError> {
        Self::from_env(env::current_dir().map_err(AgentError::State)?, start)
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
        let session_id = session::new_session_id(&workspace_root, None);
        Self::new(
            workspace_root,
            session_id,
            ModelRegistry::builtin(),
            target,
            Arc::new(|_| None),
            Some(max_tokens),
            client.into(),
        )
    }

    fn new(
        workspace_root: PathBuf,
        session_id: String,
        registry: ModelRegistry,
        target: ModelTarget,
        env: EnvLookup,
        max_tokens_override: Option<u32>,
        client: HttpClient,
    ) -> Self {
        let mut service = Self {
            control: TaskControl::default(),
            state_lock: None,
            workspace_root,
            session_id,
            session_name: None,
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
            context_window_override: None,
            compaction_threshold_override: None,
            max_tokens: DEFAULT_MAX_TOKENS,
            tools: Vec::new(),
            subagents: SubagentCatalog::builtin(),
            settings: Arc::new(Settings::default()),
            client,
            warnings: target.warnings,
            last_measured_usage: None,
            estimate_scale: 1.0,
            session_usage: Default::default(),
            system_prompt: String::new(),
            project_instructions: None,
            instructions_problem: None,
            web_search: None,
            sandbox: SandboxCheck::Passed,
        };
        service.context_window_override = service.limit_override("CONTEXT_WINDOW_TOKENS");
        service.compaction_threshold_override =
            service.limit_override("COMPACTION_TOKEN_THRESHOLD");
        if service.max_tokens_override.is_none() {
            service.max_tokens_override = service.limit_override("MAX_TOKENS");
        }
        service.apply_model_limits();
        if let Some(problem) = service.limit_problem() {
            service.warnings.push(problem);
        }
        service.refresh_system_prompt();
        service
    }

    /// A positive token count from the environment, read through the
    /// injected lookup. A value that is not one is reported and ignored.
    fn limit_override(&mut self, name: &str) -> Option<u32> {
        let raw = (self.env)(name)?;
        match raw.trim().parse::<u32>() {
            Ok(value) if value > 0 => Some(value),
            _ => {
                self.warnings.push(format!(
                    "{name} must be a positive integer; '{raw}' is ignored"
                ));
                None
            }
        }
    }

    pub fn from_env(workspace_root: PathBuf, start: SessionStart) -> Result<Self, AgentError> {
        let sandbox = SandboxCheck::start(workspace_root.clone());
        let client = HttpClient::start();
        let lookup: EnvLookup = Arc::new(|key| env::var(key).ok());
        let registry = ModelRegistry::load(lookup.as_ref())?;
        // Read from the same lookup as the other limits, in `new`.
        let max_tokens_override = None;
        settings::ensure_config_outside(&workspace_root, lookup.as_ref())?;
        let (settings, settings_warnings) = Settings::load(lookup.as_ref())?;

        let mut service = Self::open(
            workspace_root,
            start,
            registry,
            lookup,
            max_tokens_override,
            client,
        )?;
        service.warnings.extend(settings_warnings);
        service.warnings.extend(settings.summary());
        service.install_settings(settings);
        service.sandbox = sandbox;
        Ok(service)
    }

    /// Waits for the startup sandbox check. After a failure no turn can run.
    pub fn wait_for_sandbox(&mut self) -> Result<(), AgentError> {
        self.sandbox.settle(true);
        match &self.sandbox {
            SandboxCheck::Failed(message) => Err(Error::other(message.clone()).into()),
            _ => Ok(()),
        }
    }

    /// The startup sandbox check's result, or `None` while it runs.
    pub fn sandbox_status(&mut self) -> Option<Result<(), String>> {
        self.sandbox.settle(false);
        match &self.sandbox {
            SandboxCheck::Running(_) => None,
            SandboxCheck::Passed => Some(Ok(())),
            SandboxCheck::Failed(message) => Some(Err(message.clone())),
        }
    }

    /// Makes `settings` the ones tools and commands follow.
    pub(crate) fn install_settings(&mut self, mut settings: Settings) {
        // The tool is offered only when its key is set too; the settings say
        // which, so every list of tools agrees.
        self.web_search = match &settings.web_search {
            None => None,
            Some(search) => match crate::tools::web_search::WebSearch::new(search, &*self.env) {
                Ok(search) => Some(search),
                Err(warning) => {
                    self.warnings.push(warning);
                    settings.web_search = None;
                    None
                }
            },
        };
        self.settings = Arc::new(settings);
        self.apply_model_limits();
        self.refresh_system_prompt();
    }

    /// Rebuilds the system prompt from the settings and the project's
    /// instruction file. A file that cannot be used is reported, not fatal.
    fn refresh_system_prompt(&mut self) {
        if let Err(warning) = self.read_instructions() {
            self.warnings.push(warning);
        }
    }

    /// Reads the root's instruction file again and rebuilds the system
    /// prompt from it. `Err` when the file exists but cannot be used, which
    /// leaves the prompt without it.
    fn read_instructions(&mut self) -> Result<(), String> {
        let found = instructions::project_instructions(&self.workspace_root);
        self.project_instructions = found.clone().unwrap_or(None);
        self.instructions_problem = found.clone().err();
        self.system_prompt = instructions::system_prompt(
            DEFAULT_SYSTEM_PROMPT,
            &self.settings,
            self.project_instructions.as_deref(),
        );
        found.map(drop)
    }

    /// Before each turn: picks up an edited, added or removed instruction
    /// file, and says so. The prompt changes only when the file did, so the
    /// prompt cache is kept otherwise.
    fn reload_instructions(&mut self) -> Option<String> {
        let before = self.project_instructions.clone();
        let reported = self.instructions_problem.clone();
        let result = self.read_instructions();
        match (result, &before, &self.project_instructions) {
            (Err(warning), _, _) => (reported.as_ref() != Some(&warning)).then_some(warning),
            (Ok(()), old, new) if old == new => None,
            (Ok(()), _, None) => {
                Some("The project instruction file was removed; it no longer applies".to_string())
            }
            (Ok(()), _, Some(_)) => Some(
                "The project instruction file changed; the new version applies from this turn"
                    .to_string(),
            ),
        }
    }

    /// Opens the session `start` names and selects its model: the alias
    /// saved with `/model`, then `MODEL`, then the registry default. A new
    /// session takes the saved alias of the most recently saved one. A
    /// candidate that cannot be built is skipped with a warning; only the
    /// last one is fatal. History is stripped of thinking when the model
    /// differs from the saved one.
    pub(crate) fn open(
        workspace_root: PathBuf,
        start: SessionStart,
        registry: ModelRegistry,
        lookup: EnvLookup,
        max_tokens_override: Option<u32>,
        client: impl Into<HttpClient>,
    ) -> Result<Self, AgentError> {
        let state_lock = crate::agent::state::lock_workspace(&workspace_root)?;
        session::migrate_legacy_state(&workspace_root)?;
        let sessions = session::scan_sessions(&workspace_root)?;
        let mut warnings = Vec::new();
        let resumed = match &start {
            SessionStart::New => None,
            SessionStart::Continue => {
                // An unreadable latest session fails to load below rather
                // than being passed over for an older one.
                let latest = sessions
                    .iter()
                    .find(|session| !session.readable || session.message_count > 0);
                if latest.is_none() {
                    warnings.push(
                        "No saved session to continue in this workspace; started a new one"
                            .to_string(),
                    );
                }
                latest
            }
            SessionStart::Resume(query) => Some(session::resolve_session(&sessions, query)?),
        };
        let (session_id, state) = match resumed {
            Some(session) => (
                session.id.clone(),
                load_json_file::<ConversationState>(&session::session_file(
                    &workspace_root,
                    &session.id,
                    CONVERSATION_STATE_FILE_NAME,
                ))?,
            ),
            None => (
                session::new_session_id(&workspace_root, None),
                ConversationState {
                    active_model: sessions
                        .first()
                        .and_then(|session| session.active_model.clone()),
                    ..ConversationState::default()
                },
            ),
        };

        warnings.extend(crate::execution::missing_tool_programs());
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
            session_id,
            registry,
            target,
            lookup,
            max_tokens_override,
            client.into(),
        );
        service.state_lock = Some(state_lock);
        let (user_types, subagent_models, mut agent_warnings) = service.load_subagents();
        service.warnings.append(&mut agent_warnings);
        service.install_subagents(SubagentCatalog::new(user_types, subagent_models));
        let repaired = service.apply_state(state)?;
        if let Some(alias) = &saved
            && *alias == service.active_model.alias
        {
            service.selected_model = Some(alias.clone());
        }
        // A new session is written once it has something to save. A resumed
        // one is written when loading repaired it or dropped its model, or to
        // make it the newest so that `-c` continues it. Rewriting an unchanged
        // newest session would cost a full write and sync on every launch.
        if let Some(session) = resumed
            && (repaired
                || service.selected_model != saved
                || sessions.first().map(|newest| &newest.id) != Some(&session.id))
        {
            service.persist_state()?;
        }
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
        self.tools = crate::tools::tools_with_settings(&self.settings);
        if let Some(agent) = self
            .tools
            .iter_mut()
            .find(|tool| tool.name() == AGENT_TOOL_NAME)
        {
            *agent = self.subagents.tool_definition();
        }
    }

    /// User subagent types and every registered model whose provider can
    /// be built, for subagents to run on.
    fn load_subagents(&self) -> (Vec<SubagentType>, Vec<SubagentModel>, Vec<String>) {
        let mut models = Vec::new();
        let mut unavailable = Vec::new();
        for config in &self.registry.models {
            let built = self
                .registry
                .provider(&config.provider)
                .ok_or_else(|| format!("unknown provider '{}'", config.provider))
                .and_then(|provider| {
                    build_provider(provider, config, self.env.as_ref())
                        .map_err(|err| err.to_string())
                });
            match built {
                Ok(provider) => models.push(SubagentModel {
                    provider,
                    max_tokens: config
                        .max_output_tokens
                        .or(self.max_tokens_override)
                        .unwrap_or(DEFAULT_MAX_TOKENS),
                    config: config.clone(),
                }),
                Err(reason) => unavailable.push((config.alias.clone(), reason)),
            }
        }
        let (types, warnings) =
            load_user_types(
                self.env.as_ref(),
                &self.registry,
                &|config| match unavailable.iter().find(|(alias, _)| *alias == config.alias) {
                    Some((_, reason)) => Err(reason.clone()),
                    None => Ok(()),
                },
            );
        (types, models, warnings)
    }

    /// Makes `catalog` the types the `agent` tool offers.
    fn install_subagents(&mut self, catalog: SubagentCatalog) {
        self.subagents = catalog;
        self.apply_model_limits();
    }

    fn model_choice(&self, model: &ModelConfig) -> ModelChoice {
        let unavailable = match self.registry.provider(&model.provider) {
            None => Some(format!("unknown provider '{}'", model.provider)),
            Some(provider) => provider.api_key(&*self.env).err(),
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
        let previous = self.install(target);
        let previous_selection = self.selected_model.replace(alias.to_string());
        if let Err(err) = self.persist_state() {
            self.install(previous);
            self.selected_model = previous_selection;
            return Err(err.into());
        }
        // The last measurement described another model's prompt.
        self.last_measured_usage = None;
        self.estimate_scale = 1.0;
        warnings.extend(self.provider.startup_warnings());
        warnings.extend(self.limit_problem());
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
        let budget = self.context_budget();
        let history = history_input_tokens(
            &self.messages,
            &self.summary_state,
            &self.active_model.id,
            budget,
            &self.tools,
        );
        let input_budget = budget.input_budget_tokens();
        Ok((history >= self.compaction_threshold(input_budget)).then_some((history, input_budget)))
    }

    fn compaction_threshold(&self, input_budget: u32) -> u32 {
        self.compaction_threshold_override.unwrap_or_else(|| {
            compaction_threshold_tokens(input_budget, self.active_model.compact_at_tokens)
        })
    }

    /// The window in effect: `CONTEXT_WINDOW_TOKENS`, then the model's.
    fn context_window(&self) -> Option<u32> {
        self.context_window_override
            .or(self.active_model.context_window)
    }

    /// Why no message can be sent with the current limits: an output limit
    /// as large as the window leaves no room for the prompt, and every
    /// message would be refused as too long.
    fn limit_problem(&self) -> Option<String> {
        let window = self
            .context_window()
            .unwrap_or(crate::constants::DEFAULT_CONTEXT_WINDOW_TOKENS);
        (self.max_tokens >= window).then(|| {
            format!(
                "The output limit of {} tokens leaves no room for input in the {window}-token context window of '{}'; lower MAX_TOKENS or the model's max_output_tokens, or raise CONTEXT_WINDOW_TOKENS",
                self.max_tokens, self.active_model.alias
            )
        })
    }

    fn context_budget(&self) -> crate::agent::context::ContextBudget {
        resolve_context_budget(self.context_window(), self.max_tokens)
            .with_scale(self.estimate_scale)
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

    /// Continues the most recently saved session, as a restart with
    /// `SessionStart::Continue` does.
    #[cfg(test)]
    fn restore_state(&mut self) -> std::io::Result<()> {
        self.ensure_state_lock()?;
        session::migrate_legacy_state(&self.workspace_root)?;
        if let Some(latest) = session::scan_sessions(&self.workspace_root)?.first() {
            self.session_id = latest.id.clone();
        }
        let state = load_json_file::<ConversationState>(&self.conversation_path())?;
        self.apply_state(state)?;
        self.persist_state()
    }

    /// The ID that resumes this conversation.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Name given with `/rename`.
    pub fn session_name(&self) -> Option<&str> {
        self.session_name.as_deref()
    }

    /// Names this session so that `-r <name>` and `/resume <name>` open it.
    /// Surrounding whitespace is dropped.
    pub fn rename_session(&mut self, name: &str) -> std::io::Result<()> {
        self.ensure_state_lock()?;
        let name = session::validate_session_name(name)?;
        let previous = self.session_name.replace(name);
        if let Err(err) = self.persist_state() {
            self.session_name = previous;
            return Err(err);
        }
        Ok(())
    }

    /// Reverts Socium's most recent edit, or its most recent edit of
    /// `path`, when the files still hold what Socium wrote. The model is
    /// told in the conversation, so it does not build on the reverted text.
    /// Returns the paths restored.
    pub fn undo(&mut self, path: Option<&str>) -> Result<Vec<String>, AgentError> {
        self.ensure_state_lock()?;
        let restored = match path {
            Some(path) => crate::tools::edit_history::undo(&self.workspace_root, path)
                .map(|()| vec![path.to_string()]),
            None => crate::tools::edit_history::undo_last(&self.workspace_root),
        }
        .map_err(std::io::Error::other)?;
        self.messages.push(Message::user_text(format!(
            "(The user undid your last edit of {}; those files are back to their content before it.)",
            restored.join(", ")
        )));
        self.persist_state()?;
        Ok(restored)
    }

    /// The prompts this conversation can go back to, oldest first. Notes
    /// added by `/undo` are not prompts.
    pub fn rewind_points(&self) -> Vec<RewindPoint> {
        // Unreadable history lists no files; reverting them reports why.
        let edits =
            crate::tools::edit_history::session_edits(&self.workspace_root, &self.session_id)
                .unwrap_or_default();
        self.events
            .iter()
            .filter(|event| event["type"] == "turn_started")
            .filter_map(|event| {
                let turn_id = usize::try_from(event["turn_id"].as_u64()?).ok()?;
                match self.messages.get(turn_id)? {
                    Message {
                        role: Role::User,
                        content: MessageContent::Text(prompt),
                        ..
                    } => Some(RewindPoint {
                        turn_id,
                        prompt: prompt.clone(),
                        changed_files: edits
                            .iter()
                            .filter(|(edit_turn, _)| *edit_turn >= turn_id)
                            .map(|(_, path)| path.clone())
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter()
                            .collect(),
                    }),
                    _ => None,
                }
            })
            .collect()
    }

    /// Goes back to just before the prompt at `turn_id`, between turns:
    /// puts the files Socium's edits changed from it on back, drops the
    /// conversation from it, or both, as `mode` says. Files are reverted
    /// first, and only when every one still holds what Socium wrote.
    pub fn rewind(&mut self, turn_id: usize, mode: RewindMode) -> Result<Rewound, AgentError> {
        self.ensure_state_lock()?;
        let Some(point) = self
            .rewind_points()
            .into_iter()
            .find(|point| point.turn_id == turn_id)
        else {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("No prompt of this conversation starts at message {turn_id}"),
            )
            .into());
        };
        let restored_files = match mode {
            RewindMode::Conversation => Vec::new(),
            RewindMode::ConversationAndFiles | RewindMode::Files => {
                crate::tools::edit_history::revert_since(
                    &self.workspace_root,
                    &self.session_id,
                    turn_id,
                )
                .map_err(Error::other)?
            }
        };
        if mode == RewindMode::Files {
            return self.note_reverted_files(point.prompt, restored_files);
        }
        let mut warnings = Vec::new();
        if let Err(err) = self.drop_turns_from(turn_id) {
            if restored_files.is_empty() {
                return Err(err.into());
            }
            return Err(Error::other(format!(
                "The files were restored ({}), but the conversation could not be rewound: {err}",
                restored_files.join(", ")
            ))
            .into());
        }
        if let Err(err) = crate::activity_log::drop_turns_from(&self.activity_log_path(), turn_id) {
            warnings.push(format!(
                "The Activity of the dropped turns is still saved: {err}"
            ));
        }
        if mode == RewindMode::Conversation
            && let Err(err) = crate::tools::edit_history::forget_since(
                &self.workspace_root,
                &self.session_id,
                turn_id,
            )
        {
            warnings.push(format!(
                "A later /rewind may revert the edits of the dropped turns: {err}"
            ));
        }
        Ok(Rewound {
            prompt: point.prompt,
            warning: (!warnings.is_empty()).then(|| warnings.join("; ")),
            restored_files,
        })
    }

    /// Drops the messages and events of the turns from `turn_id` on and
    /// saves the conversation, or changes nothing.
    fn drop_turns_from(&mut self, turn_id: usize) -> std::io::Result<()> {
        let previous_summary = self.summary_state.clone();
        let previous_usage = self.last_measured_usage.take();
        let mut dropped_messages = self.messages.split_off(turn_id);
        let cut = self
            .events
            .iter()
            .position(|event| {
                event["turn_id"]
                    .as_u64()
                    .is_some_and(|id| id >= turn_id as u64)
            })
            .unwrap_or(self.events.len());
        let mut dropped_events = self.events.split_off(cut);
        match &mut self.summary_state.summary {
            Some(summary) if summary.covered_messages > turn_id => {
                self.summary_state = ConversationSummaryState::default();
            }
            Some(summary) => {
                if let Some(from) = &mut summary.thinking_valid_from {
                    *from = (*from).min(turn_id);
                }
            }
            None => {}
        }
        if let Err(err) = self.persist_state() {
            self.messages.append(&mut dropped_messages);
            self.events.append(&mut dropped_events);
            self.summary_state = previous_summary;
            self.last_measured_usage = previous_usage;
            return Err(err);
        }
        Ok(())
    }

    /// Tells the model, in the kept conversation, that the files its turns
    /// from `prompt` on changed are back as they were.
    fn note_reverted_files(
        &mut self,
        prompt: String,
        restored_files: Vec<String>,
    ) -> Result<Rewound, AgentError> {
        if restored_files.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "Socium changed no files from this prompt on",
            )
            .into());
        }
        let excerpt = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        let excerpt = match excerpt.char_indices().nth(80) {
            Some((end, _)) => format!("{}…", &excerpt[..end]),
            None => excerpt,
        };
        self.messages.push(Message::user_text(format!(
            "(The user reverted the files you changed since their prompt \"{excerpt}\": {}; those files are back to their content before it.)",
            restored_files.join(", ")
        )));
        if let Err(err) = self.persist_state() {
            self.messages.pop();
            return Err(Error::other(format!(
                "The files were restored ({}), but the conversation could not be saved: {err}",
                restored_files.join(", ")
            ))
            .into());
        }
        Ok(Rewound {
            prompt,
            warning: None,
            restored_files,
        })
    }

    /// Permanently removes another saved session, named by its full ID.
    pub fn delete_session(&mut self, id: &str) -> std::io::Result<()> {
        self.ensure_state_lock()?;
        if id == self.session_id {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "The open session cannot be deleted; /clear or /resume another one first",
            ));
        }
        session::delete_session_dir(&self.workspace_root, id)
    }

    /// Whether the conversation has messages, so it can be resumed.
    pub fn has_saved_messages(&self) -> bool {
        !self.messages.is_empty()
    }

    pub(crate) fn conversation_path(&self) -> PathBuf {
        session::session_file(
            &self.workspace_root,
            &self.session_id,
            CONVERSATION_STATE_FILE_NAME,
        )
    }

    /// Loads `state` and reports whether loading had to repair it.
    fn apply_state(&mut self, state: ConversationState) -> std::io::Result<bool> {
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
        self.session_name = state.name;
        let mut repaired = crate::agent::turn::repair_incomplete_tools(&mut self.messages);
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
            repaired = true;
        }
        *self.session_usage.lock().unwrap_or_else(|e| e.into_inner()) = SessionUsage::default();
        Ok(repaired)
    }

    /// Starts a new session with the current model. The previous one stays
    /// saved and can be resumed by its ID.
    pub fn clear(&mut self) -> std::io::Result<()> {
        self.ensure_state_lock()?;
        self.session_id = session::new_session_id(&self.workspace_root, Some(&self.session_id));
        self.session_name = None;
        self.messages.clear();
        self.events.clear();
        self.summary_state = ConversationSummaryState::default();
        self.last_measured_usage = None;
        *self.session_usage.lock().unwrap_or_else(|e| e.into_inner()) = SessionUsage::default();
        Ok(())
    }

    /// Tokens this conversation used since it was opened, and their cost
    /// when every model used has a `price` in models.toml.
    pub fn session_usage(&self) -> (TokenUsage, Option<f64>) {
        let usage = self.session_usage.lock().unwrap_or_else(|e| e.into_inner());
        let cost = usage.cost(|model| {
            self.registry
                .models
                .iter()
                .find(|config| config.id == model)
                .and_then(|config| config.price)
        });
        (usage.total(), cost)
    }

    /// Model calls this session whose provider reported no usage, so
    /// `session_usage` is a lower bound.
    pub fn unreported_model_calls(&self) -> usize {
        self.session_usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unreported()
    }

    /// Other saved sessions in this workspace, most recently saved first.
    pub fn list_sessions(&mut self) -> std::io::Result<Vec<SessionSummary>> {
        self.ensure_state_lock()?;
        Ok(session::scan_sessions(&self.workspace_root)?
            .into_iter()
            .filter(|session| {
                session.id != self.session_id && (session.message_count > 0 || !session.readable)
            })
            .collect())
    }

    /// Switches to the saved session `query` names, between turns. Its
    /// saved `/model` choice is applied when that model can start;
    /// otherwise the current model stays and the history loses its
    /// thinking, as at startup. On failure the current session stays open.
    pub fn resume_session(&mut self, query: &str) -> Result<ModelSwitch, AgentError> {
        self.ensure_state_lock()?;
        let sessions = session::scan_sessions(&self.workspace_root)?;
        let target = session::resolve_session(&sessions, query)?;
        if target.id == self.session_id {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("Session {} is already open", target.id),
            )
            .into());
        }
        let state = load_json_file::<ConversationState>(&session::session_file(
            &self.workspace_root,
            &target.id,
            CONVERSATION_STATE_FILE_NAME,
        ))?;
        let saved = state.active_model.clone();
        let previous_id = std::mem::replace(&mut self.session_id, target.id.clone());
        let previous_messages = std::mem::take(&mut self.messages);
        let previous_events = std::mem::take(&mut self.events);
        let previous_summary = std::mem::take(&mut self.summary_state);
        let previous_selection = self.selected_model.take();
        let previous_name = self.session_name.take();
        let previous_usage = self.last_measured_usage.take();
        let mut previous_model = None;
        let mut warnings = Vec::new();
        let result = self.apply_state(state).and_then(|_| {
            match saved {
                Some(alias) if alias == self.active_model.alias => {
                    self.selected_model = Some(alias);
                }
                Some(alias) => {
                    let current = self.active_model.alias.clone();
                    let built = match self.registry.find(&alias).cloned() {
                        Some(model) => ModelTarget::build(&self.registry, model, self.env.as_ref())
                            .map_err(|err| format!("Model '{alias}' is unavailable: {err}")),
                        None => Err(format!("Saved model '{alias}' is no longer registered")),
                    };
                    match built {
                        Ok(mut target) => {
                            warnings.append(&mut target.warnings);
                            previous_model = Some(self.install(target));
                            self.selected_model = Some(alias);
                            warnings.extend(self.provider.startup_warnings());
                        }
                        Err(reason) => {
                            warnings.push(format!("{reason}; staying on '{current}'"));
                        }
                    }
                }
                None => {}
            }
            self.persist_state()
        });
        if let Err(err) = result {
            if let Some(model) = previous_model {
                self.install(model);
            }
            self.session_id = previous_id;
            self.messages = previous_messages;
            self.events = previous_events;
            self.summary_state = previous_summary;
            self.selected_model = previous_selection;
            self.session_name = previous_name;
            self.last_measured_usage = previous_usage;
            return Err(err.into());
        }
        let needs_compaction = match self.history_pressure()? {
            Some((history, budget)) => {
                warnings.push(format!(
                    "History is about {history} tokens, near or over {}'s input budget of {budget}; compact it before continuing",
                    self.active_model.alias
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

    /// Activity of the most recent saved turns, oldest first.
    /// The plan this conversation's `update_plan` calls left, or `None`
    /// when it has none. Derived from the history on every call, so rewind,
    /// resume and /clear need no plan code of their own.
    pub fn plan(&self) -> Option<PlanView> {
        crate::plan::replay(&self.messages)
            .plan
            .map(|plan| plan.view())
    }

    pub fn recent_activity(&self) -> Vec<ActivityEvent> {
        crate::activity_log::load_recent(
            &self.activity_log_path(),
            crate::activity_log::REPLAY_TURNS,
        )
    }

    fn activity_log_path(&self) -> PathBuf {
        session::session_file(
            &self.workspace_root,
            &self.session_id,
            ACTIVITY_LOG_FILE_NAME,
        )
    }

    /// Whether sandboxed commands run without asking
    /// (`approval.commands = "auto"`).
    pub fn commands_run_without_asking(&self) -> bool {
        self.settings.approval.commands == settings::CommandApproval::Auto
    }

    /// Tolerated configuration problems to show once at startup.
    pub fn startup_warnings(&self) -> Vec<String> {
        let mut warnings = self.warnings.clone();
        warnings.extend(self.provider.startup_warnings());
        // The current model is also a subagent model; its warnings name
        // its alias, so a repeat is the same text.
        for warning in self.subagents.startup_warnings() {
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
        warnings
    }

    pub fn transcript_entries(&self) -> Vec<TranscriptEntry> {
        transcript::transcript_entries(&self.messages, &self.events)
    }

    /// Subagents started in this session, most recently updated first.
    pub fn subagent_runs(&self) -> std::io::Result<Vec<SubagentSummary>> {
        Ok(
            crate::agent::subagent::record::list(&self.workspace_root, &self.session_id)?
                .iter()
                .map(|record| record.summary())
                .collect(),
        )
    }

    /// A subagent started in this session and its conversation, prompts
    /// and tool results included.
    pub fn subagent_transcript(
        &self,
        id: &str,
    ) -> std::io::Result<(SubagentSummary, Vec<TranscriptEntry>)> {
        let record =
            crate::agent::subagent::record::load(&self.workspace_root, &self.session_id, id)?
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::NotFound,
                        format!("No saved subagent '{id}' in this session"),
                    )
                })?;
        Ok((
            record.summary(),
            transcript::transcript_entries(&record.messages, &record.tool_events),
        ))
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
        self.wait_for_sandbox()?;
        self.ensure_state_lock()?;
        if let Some(problem) = self.limit_problem() {
            return Err(Error::new(ErrorKind::InvalidInput, problem).into());
        }
        crate::agent::turn::repair_incomplete_tools(&mut self.messages);
        let turn_id = self.messages.len();
        let clock = ActivityClock::start(turn_id);
        let mut recorder = ActivityRecorder::new(self.activity_log_path());
        let session_usage = self.session_usage.clone();
        let mut on_progress = move |event: AgentProgress| {
            let warning = match &event {
                AgentProgress::Activity(activity) => {
                    session_usage
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .observe(activity);
                    recorder.observe(activity)
                }
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
        if let Some(message) = self.reload_instructions() {
            emit(&mut on_progress, ActivityKind::Warning { message });
        }
        let mut plan = crate::plan::PlanLedger::resume(&self.messages);
        for message in plan.take_replay_warnings() {
            emit(&mut on_progress, ActivityKind::Warning { message });
        }
        if self.should_auto_compact()? {
            on_progress(AgentProgress::CompactionStarted);
            emit(&mut on_progress, ActivityKind::CompactionStarted);
            match self.run_compaction().await {
                Ok(report) => {
                    if let Some(reason) = &report.fallback_reason {
                        emit(
                            &mut on_progress,
                            ActivityKind::Warning {
                                message: reason.clone(),
                            },
                        );
                    }
                    on_progress(AgentProgress::CompactionFinished {
                        covered_messages: report.covered_messages,
                        used_llm: report.used_llm,
                    });
                    emit(&mut on_progress, report.finished);
                }
                // Compaction must never block the conversation; the
                // budget-based trimming below still bounds the prompt.
                Err(AgentError::NothingToCompact) => {}
                Err(err) => emit(
                    &mut on_progress,
                    ActivityKind::Warning {
                        message: format!("Auto-compaction failed: {err}"),
                    },
                ),
            }
        }
        let prepared_context = prepare_context_messages(
            &self.messages,
            &self.summary_state,
            self.context_budget(),
            &self.active_model.id,
            &self.tools,
        )?;
        let initial_len = prepared_context.messages.len();
        let mut context_messages = prepared_context.messages;
        let edit_origin = crate::tools::edit_history::EditOrigin {
            session_id: self.session_id.clone(),
            turn_id,
        };
        let client = self.client.get()?;
        let turn_context = TurnContext {
            provider: self.provider.as_ref(),
            client: &client,
            model: &self.active_model.id,
            system: &self.system_prompt,
            project_instructions: self.project_instructions.as_deref().unwrap_or(""),
            context_window: self.context_window(),
            max_tokens: self.max_tokens,
            estimate_scale: self.estimate_scale,
            compaction: (
                self.compaction_provider
                    .as_deref()
                    .unwrap_or(self.provider.as_ref()),
                &self.compaction_model,
            ),
            rounds_done: 0,
            tools: &self.tools,
            subagents: &self.subagents,
            workspace_root: &self.workspace_root,
            edit_origin: Some(&edit_origin),
            session_id: &self.session_id,
            control: &self.control,
            settings: &self.settings,
            clock,
            web_search: self.web_search.as_ref(),
        };
        let raw_len = self.messages.len();
        let active_model = self.selected_model.clone();
        let session_name = self.session_name.clone();
        self.summary_state = prepared_context.summary_state;
        let path = &self.conversation_path();
        let raw_messages = &mut self.messages;
        let summary = &self.summary_state;
        let events = &mut self.events;
        let mut checkpoint = |context: &[Message],
                              event: serde_json::Value|
         -> std::io::Result<()> {
            raw_messages.truncate(raw_len);
            raw_messages.extend_from_slice(&context[initial_len..]);
            events.push(serde_json::json!({"sequence":events.len(),"turn_id":turn_id,"message_offset":raw_messages.len(),"event":event}));
            session::save_conversation(
                path,
                &ConversationStateRef {
                    schema_version: 1,
                    messages: raw_messages,
                    events,
                    summary_state: summary,
                    active_model: active_model.as_deref(),
                    name: session_name.as_deref(),
                },
            )
        };
        let result = run_agent_turn(
            &turn_context,
            &mut context_messages,
            Some(&mut plan),
            &mut on_progress,
            &mut checkpoint,
        )
        .await;
        // The live plan and the replay of what was saved must agree. A failed
        // turn may have applied a call whose result was not saved, so only a
        // finished turn is checked.
        if result.is_ok() {
            debug_assert_eq!(
                plan.plan(),
                crate::plan::replay(&self.messages).plan.as_ref(),
                "the live plan and its replay diverged"
            );
        }
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
        if let Err(AgentError::ContextFull(_)) = &result {
            // The window filled in the middle of the turn. Compacting now
            // makes the next message fit; the user decides how to go on.
            self.last_measured_usage = None;
            let compacted = match self.run_compaction().await {
                Ok(report) => {
                    on_progress(AgentProgress::CompactionFinished {
                        covered_messages: report.covered_messages,
                        used_llm: report.used_llm,
                    });
                    on_progress(AgentProgress::Activity(clock.event(0, report.finished)));
                    "The history was compacted; ask Socium to continue.".to_string()
                }
                Err(err) => {
                    format!("Compaction also failed ({err}); start a new session with /clear.")
                }
            };
            if let Err(AgentError::ContextFull(message)) = result {
                return Err(AgentError::ContextFull(format!("{message}. {compacted}")));
            }
        }
        let outcome = result?;
        if outcome.last_usage.is_some() {
            self.last_measured_usage = outcome.last_usage;
        }
        if let Some(scale) = outcome.estimate_scale {
            self.estimate_scale = scale;
        }
        Ok(AgentReply {
            text: outcome.assistant_text_chunks.join("\n"),
            notices: outcome.notices,
        })
    }

    pub fn context_metrics(&self, pending_user_input: &str) -> std::io::Result<ContextMetrics> {
        let mut metrics = context_metrics_for_input(
            &self.messages,
            pending_user_input,
            &self.summary_state,
            self.context_budget(),
            &self.active_model.id,
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
        let report = self.run_compaction().await?;
        // Outside a turn nothing else observes the event; count its call.
        self.session_usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .observe(&ActivityClock::start(0).event(0, report.finished.clone()));
        Ok(report)
    }

    fn should_auto_compact(&self) -> Result<bool, AgentError> {
        let Some(usage) = self.last_measured_usage else {
            return Ok(false);
        };
        let budget = self.context_budget();
        let threshold = self.compaction_threshold(budget.input_budget_tokens());
        Ok(usage.total_input_tokens() >= threshold)
    }

    async fn run_compaction(&mut self) -> Result<CompactionReport, AgentError> {
        let compaction = compact_history(
            (
                self.compaction_provider
                    .as_deref()
                    .unwrap_or(self.provider.as_ref()),
                &self.compaction_model,
            ),
            &self.client.get()?,
            &self.control,
            &self.active_model.id,
            &self.messages,
            &self.summary_state,
        )
        .await?;
        let finished = compaction.finished_event();
        self.summary_state = compaction.summary_state;
        self.persist_state()?;

        // The old measurement described the pre-compaction prompt; drop it
        // so metrics reflect the compacted context until the next call.
        self.last_measured_usage = None;
        let metrics = self.context_metrics("")?;
        Ok(CompactionReport {
            covered_messages: compaction.covered_messages,
            retained_messages: self.messages.len() - compaction.covered_messages,
            used_llm: compaction.fallback_reason.is_none(),
            fallback_reason: compaction.fallback_reason,
            metrics,
            finished,
        })
    }

    fn persist_state(&self) -> std::io::Result<()> {
        session::save_conversation(
            &self.conversation_path(),
            &ConversationStateRef {
                schema_version: 1,
                messages: &self.messages,
                events: &self.events,
                summary_state: &self.summary_state,
                active_model: self.selected_model.as_deref(),
                name: self.session_name.as_deref(),
            },
        )
    }
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod security_tests;

#[cfg(test)]
mod plan_tests;

#[cfg(test)]
mod tests {
    use crate::agent::types::{ContentBlock, Message, MessageContent};
    use crate::constants::CONVERSATION_STATE_FILE_NAME;
    use crate::providers::TokenUsage;
    use crate::providers::anthropic::AnthropicProvider;
    use crate::test_support::{ScriptedProvider, make_temp_dir, text_provider_response};
    use crate::{AgentService, TranscriptEntry, TranscriptRole};
    use serde_json::json;

    #[test]
    fn transcript_entries_include_text_and_tool_results() {
        let workspace = make_temp_dir("transcript");
        let messages = vec![
            Message::user_text("hello".to_string()),
            Message::assistant_blocks(vec![
                ContentBlock::Text {
                    text: "answer".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "src/lib.rs"}),
                },
            ]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "Path: src/lib.rs".to_string(),
                is_error: None,
            }]),
        ];

        let mut service = AgentService::with_provider(
            workspace.path().to_path_buf(),
            Box::new(AnthropicProvider::with_credentials("test", "2023-06-01")),
            "test-model".to_string(),
            128,
            reqwest::Client::builder().build().unwrap(),
        );
        service.messages = messages;

        let entries = service.transcript_entries();
        assert_eq!(
            entries,
            [
                TranscriptEntry {
                    role: TranscriptRole::User,
                    text: "hello".into()
                },
                TranscriptEntry {
                    role: TranscriptRole::Assistant,
                    text: "answer".into()
                },
                TranscriptEntry {
                    role: TranscriptRole::Tool { failed: false },
                    text: "read_file src/lib.rs\nPath: src/lib.rs".into()
                },
            ]
        );
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

    #[tokio::test]
    async fn a_model_without_tools_is_sent_none_and_answers_in_one_round() {
        let workspace = make_temp_dir("no_tools");
        let mut provider =
            ScriptedProvider::new(vec![Ok(text_provider_response("plain answer", None))]);
        provider.supports_tools = false;
        let requests = provider.recorded_requests.clone();
        let mut service =
            service_with_scripted_provider(workspace.path(), provider, Vec::new(), None);

        let reply = service.submit("hello".into()).await.unwrap();

        assert_eq!(reply.text, "plain answer");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .get("tools")
                .and_then(|tools| tools.as_array())
                .is_none_or(|tools| tools.is_empty()),
            "{}",
            requests[0]["tools"]
        );
    }

    #[tokio::test]
    async fn instruction_files_follow_edits_and_reach_the_tools_that_use_their_scope() {
        let workspace = make_temp_dir("scoped_instructions");
        let root = workspace.path();
        std::fs::create_dir_all(root.join("crates/tui")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "Root rule one.").unwrap();
        std::fs::write(root.join("crates/tui/AGENTS.md"), "TUI rule.").unwrap();
        std::fs::write(root.join("crates/tui/app.rs"), "fn main() {}\n").unwrap();
        let read = |id: &str| {
            Ok(crate::providers::ProviderResponse {
                message: crate::agent::types::AgentMessageResponse {
                    content: vec![ContentBlock::ToolUse {
                        id: id.into(),
                        name: "read_file".into(),
                        input: json!({"path": "crates/tui/app.rs"}),
                    }],
                    ..Default::default()
                },
                streamed_text: false,
                usage: None,
            })
        };
        let provider = ScriptedProvider::new(vec![
            read("r1"),
            read("r2"),
            Ok(text_provider_response("read it", None)),
            Ok(text_provider_response("ok", None)),
        ]);
        let requests = provider.recorded_requests.clone();
        let mut service = service_with_scripted_provider(root, provider, Vec::new(), None);
        service.refresh_system_prompt();

        service.submit("look at the app".into()).await.unwrap();
        std::fs::write(root.join("AGENTS.md"), "Root rule two.").unwrap();
        let mut notes = Vec::new();
        service
            .submit_with_progress("again".into(), |event| {
                if let crate::AgentProgress::Activity(crate::ActivityEvent {
                    kind: crate::ActivityKind::Warning { message },
                    ..
                }) = event
                {
                    notes.push(message);
                }
            })
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        let result = |request: &serde_json::Value, id: &str| {
            request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|m| m["content"].as_array().into_iter().flatten())
                .find(|b| b["tool_use_id"] == id)
                .map(|b| b["content"].as_str().unwrap().to_string())
                .unwrap()
        };
        let first = result(&requests[2], "r1");
        let second = result(&requests[2], "r2");
        assert!(
            first.contains("TUI rule.") && first.contains("for files under crates/tui/."),
            "{first}"
        );
        assert!(!second.contains("TUI rule."), "delivered once: {second}");
        assert!(
            requests[0]["system"]
                .as_str()
                .unwrap()
                .contains("Root rule one.")
        );
        assert!(
            requests[3]["system"]
                .as_str()
                .unwrap()
                .contains("Root rule two.")
        );
        assert!(
            notes.iter().any(|n| n.contains("instruction file changed")),
            "{notes:?}"
        );
    }

    #[tokio::test]
    async fn no_request_is_sent_when_the_output_limit_fills_the_window() {
        let workspace = make_temp_dir("limits_refused");
        let provider = ScriptedProvider::new(vec![Ok(text_provider_response("hi", None))]);
        let requests = provider.recorded_requests.clone();
        let mut service =
            service_with_scripted_provider(workspace.path(), provider, Vec::new(), None);
        service.context_window_override = Some(1_000);

        let Err(err) = service.submit("hello".into()).await else {
            panic!("the turn ran");
        };
        let err = err.to_string();

        assert!(err.contains("leaves no room for input"), "{err}");
        assert!(requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_agent_loop_searches_the_web_when_the_user_turned_it_on() {
        let (base_url, _server) = crate::providers::test_server::serve_once(
            "200 OK",
            r#"{"results":[{"title":"Tokio 2.0","url":"https://tokio.rs/blog","summary":"Released.","date":"2026-10-01T00:00:00Z"}]}"#.into(),
        )
        .await;
        let workspace = make_temp_dir("web_search");
        let provider = ScriptedProvider::new(vec![
            Ok(crate::providers::ProviderResponse {
                message: crate::agent::types::AgentMessageResponse {
                    content: vec![ContentBlock::ToolUse {
                        id: "s1".into(),
                        name: "web_search".into(),
                        input: json!({"query": "tokio release"}),
                    }],
                    ..Default::default()
                },
                streamed_text: false,
                usage: None,
            }),
            Ok(text_provider_response(
                "Tokio 2.0 is out (https://tokio.rs/blog).",
                None,
            )),
        ]);
        let requests = provider.recorded_requests.clone();
        let mut service =
            service_with_scripted_provider(workspace.path(), provider, Vec::new(), None);
        service.env =
            std::sync::Arc::new(|name| (name == "DEEPSEEK_API_KEY").then(|| "sk".to_string()));
        let (settings, _) = crate::Settings::from_toml(
            &format!("[web_search]\nenabled = true\nbase_url = \"{base_url}\""),
            None,
        )
        .unwrap();
        service.install_settings(settings);

        service
            .submit("what is new in tokio?".into())
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        let offered = requests[0]["tools"].as_array().unwrap();
        assert!(offered.iter().any(|tool| tool["name"] == "web_search"));
        let result = requests[1]["messages"][2]["content"][0]["content"]
            .as_str()
            .unwrap();
        assert!(result.contains("https://tokio.rs/blog"), "{result}");

        // Turned on without its key: not offered, and the user is told.
        service.env = std::sync::Arc::new(|_| None);
        let (settings, _) =
            crate::Settings::from_toml("[web_search]\nenabled = true", None).unwrap();
        service.install_settings(settings);
        assert!(!service.tools.iter().any(|tool| tool.name() == "web_search"));
        assert!(
            service
                .startup_warnings()
                .iter()
                .any(|w| w.contains("DEEPSEEK_API_KEY is not set"))
        );
    }

    #[tokio::test]
    async fn no_turn_starts_after_the_sandbox_check_fails() {
        let workspace = make_temp_dir("sandbox_check");
        let provider = ScriptedProvider::new(vec![Ok(text_provider_response("hi", None))]);
        let requests = provider.recorded_requests.clone();
        let mut service =
            service_with_scripted_provider(workspace.path(), provider, Vec::new(), None);
        service.sandbox =
            super::SandboxCheck::Running(std::thread::spawn(
                || Err("bwrap is missing".to_string()),
            ));

        let Err(error) = service.submit("hello".into()).await else {
            panic!("a turn ran without a sandbox");
        };

        assert!(error.to_string().contains("bwrap is missing"), "{error}");
        assert_eq!(
            service.sandbox_status(),
            Some(Err("bwrap is missing".into()))
        );
        assert!(requests.lock().unwrap().is_empty());
        assert!(service.messages.is_empty());
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
            crate::agent::state::load_json_file(&service.conversation_path()).unwrap();
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
    async fn the_summary_call_counts_toward_the_session_usage() {
        let workspace = make_temp_dir("compaction_usage");
        let used = |input, output| TokenUsage {
            input_tokens: input,
            output_tokens: output,
            ..TokenUsage::default()
        };
        let provider = ScriptedProvider::new(vec![
            Ok(text_provider_response(
                "1. User request: summary",
                Some(used(5_000, 300)),
            )),
            // The turn's own call reports nothing.
            Ok(text_provider_response("turn answer", None)),
        ]);
        let mut service = service_with_scripted_provider(
            workspace.path(),
            provider,
            plain_user_messages(30),
            Some(TokenUsage {
                input_tokens: 170_000,
                ..TokenUsage::default()
            }),
        );

        service.submit("next question".to_string()).await.unwrap();

        assert_eq!(service.session_usage().0, used(5_000, 300));
        assert_eq!(service.unreported_model_calls(), 1);
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
                message: AgentMessageResponse {
                    content: blocks,
                    ..Default::default()
                },
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
            let path = service.conversation_path();
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
        use crate::SessionStart;
        use crate::models::ModelRegistry;
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
                SessionStart::New,
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

        #[test]
        fn switching_to_a_model_without_tools_says_so() {
            let root = make_temp_dir("switch_no_tools");
            let models = format!(
                "{MODELS}\n[[models]]\nalias = \"ds-plain\"\nprovider = \"deepseek\"\nid = \"deepseek-plain\"\ntools = false\n"
            );
            let mut service = AgentService::open(
                root.path().to_path_buf(),
                SessionStart::New,
                ModelRegistry::from_toml(&models).unwrap(),
                lookup(&KEYS),
                None,
                reqwest::Client::new(),
            )
            .unwrap();
            let switch = service.switch_model("ds-plain").unwrap();
            assert!(
                switch
                    .warnings
                    .iter()
                    .any(|w| w.contains("'ds-plain' is set to take no tools")),
                "{:?}",
                switch.warnings
            );
            assert!(!service.provider.capabilities().supports_tools);
        }

        #[test]
        fn an_output_limit_that_fills_the_window_is_reported_before_any_request() {
            let root = make_temp_dir("limits_conflict");
            let mut env = KEYS.to_vec();
            // opus has no max_output_tokens, so the 64000-token default
            // meets a 50000-token window.
            env.extend([("CONTEXT_WINDOW_TOKENS", "50000"), ("MAX_TOKENS", "0")]);
            let service = open(root.path(), &env);
            let warnings = service.startup_warnings();
            assert!(
                warnings
                    .iter()
                    .any(|w| w.contains("MAX_TOKENS must be a positive integer; '0' is ignored")),
                "{warnings:?}"
            );
            assert!(
                warnings.iter().any(|w| w.contains(
                    "The output limit of 64000 tokens leaves no room for input in the 50000-token context window"
                )),
                "{warnings:?}"
            );
        }

        #[test]
        fn switching_to_a_model_whose_output_fills_the_window_says_so() {
            let root = make_temp_dir("switch_limits_conflict");
            let mut env = KEYS.to_vec();
            env.push(("CONTEXT_WINDOW_TOKENS", "40000"));
            let mut service = open(root.path(), &env);
            // ds-flash's 32000-token output fits a 40000-token window.
            let fits = service.switch_model("ds-flash").unwrap();
            assert!(
                !fits
                    .warnings
                    .iter()
                    .any(|w| w.contains("no room for input"))
            );
            let opus = service.switch_model("opus").unwrap();
            assert!(
                opus.warnings
                    .iter()
                    .any(|w| w.contains("no room for input")),
                "{:?}",
                opus.warnings
            );
        }

        #[test]
        fn limit_overrides_come_from_the_injected_environment() {
            // Review N15: these two were read with env::var, so a test
            // could not set them and they bypassed the lookup.
            let root = make_temp_dir("limit_overrides");
            let mut env = KEYS.to_vec();
            env.extend([
                ("CONTEXT_WINDOW_TOKENS", "50000"),
                ("COMPACTION_TOKEN_THRESHOLD", "lots"),
            ]);
            let service = open(root.path(), &env);
            assert_eq!(
                service.context_metrics("").unwrap().context_window_tokens,
                50_000
            );
            let warnings = service.startup_warnings();
            assert!(
                warnings
                    .iter()
                    .any(|w| w.contains("COMPACTION_TOKEN_THRESHOLD must be a positive integer")),
                "{warnings:?}"
            );
            let input_budget = service.context_budget().input_budget_tokens();
            assert_eq!(
                service.compaction_threshold(input_budget),
                crate::agent::compaction::compaction_threshold_tokens(input_budget, None)
            );
        }

        /// The most recently saved conversation; null when none is saved.
        fn saved_state(root: &std::path::Path) -> serde_json::Value {
            let Some(latest) = crate::session::scan_sessions(root)
                .unwrap()
                .into_iter()
                .next()
            else {
                return serde_json::Value::Null;
            };
            let path = crate::session::session_file(root, &latest.id, CONVERSATION_STATE_FILE_NAME);
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
        }

        /// A turn Sonnet answered with thinking.
        fn thinking_turn() -> Vec<Message> {
            vec![
                Message::user_text("hi".into()),
                Message::assistant_from(
                    vec![
                        ContentBlock::Thinking {
                            thinking: "plan".into(),
                            signature: "sig".into(),
                        },
                        ContentBlock::Text {
                            text: "answer".into(),
                        },
                    ],
                    crate::agent::types::Origin {
                        provider: "anthropic".into(),
                        model: "claude-sonnet-5-5".into(),
                    },
                ),
            ]
        }

        /// Whether the next request to the current model carries thinking.
        fn sends_thinking(service: &AgentService) -> bool {
            let sent = crate::agent::types::readable_thinking(&service.messages, &|origin| {
                service.provider.reads_thinking_from(origin)
            })
            .unwrap_or_else(|| service.messages.clone());
            has_thinking(&sent)
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
            // The history keeps Sonnet's thinking; DeepSeek is not sent it.
            assert!(has_thinking(&service.messages));
            assert!(!sends_thinking(&service));
            let state = saved_state(root.path());
            assert_eq!(state["active_model"], "ds-flash");
            assert!(state.to_string().contains("\"thinking\""));
            drop(service);

            let mut service = open(root.path(), &with_model("sonnet"));
            assert_eq!(service.active_model().alias, "ds-flash");
            assert!(service.startup_warnings().is_empty());
            // Back on Anthropic, any Claude model is sent the block.
            service.messages = thinking_turn();
            service.switch_model("opus").unwrap();
            assert!(sends_thinking(&service));
        }

        #[test]
        fn an_unavailable_saved_model_falls_back_with_a_warning_and_keeps_thinking_unsent() {
            let root = make_temp_dir("model_fallback");
            let mut service = open(root.path(), &KEYS);
            service.switch_model("ds-flash").unwrap();
            service.messages = thinking_turn();
            service.messages[1].origin = Some(crate::agent::types::Origin {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
            });
            service.persist_state().unwrap();
            drop(service);

            let service = AgentService::open(
                root.path().to_path_buf(),
                SessionStart::Continue,
                ModelRegistry::from_toml(MODELS).unwrap(),
                lookup(&[("ANTHROPIC_API_KEY", "a"), ("MODEL", "sonnet")]),
                None,
                reqwest::Client::new(),
            )
            .unwrap();
            assert_eq!(service.active_model().alias, "sonnet");
            let warnings = service.startup_warnings();
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].contains("'ds-flash'"));
            assert!(warnings[0].contains("DEEPSEEK_API_KEY is not set"));
            assert!(has_thinking(&service.messages));
            assert!(!sends_thinking(&service));
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
                SessionStart::New,
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
        fn settings_a_subagent_model_ignores_are_warned_about_once() {
            let root = make_temp_dir("subagent_model_warnings");
            let registry = ModelRegistry::from_toml(
                r#"
default = "main"
[providers.p]
wire = "anthropic"
base_url = "https://p.example"
api_key_env = "P_KEY"

[[models]]
alias = "main"
provider = "p"
id = "main-id"
features = ["effort"]

[[models]]
alias = "plain"
provider = "p"
id = "plain-id"

[[models]]
alias = "plain-too"
provider = "p"
id = "plain-id"
"#,
            )
            .unwrap();
            let service = AgentService::open(
                root.path().to_path_buf(),
                SessionStart::New,
                registry,
                lookup(&[("P_KEY", "k"), ("EFFORT", "high"), ("MODEL", "plain")]),
                None,
                reqwest::Client::new(),
            )
            .unwrap();

            let warnings = service.startup_warnings();
            let effort = warnings
                .iter()
                .filter(|warning| warning.contains("EFFORT is ignored"))
                .collect::<Vec<_>>();
            assert_eq!(
                effort,
                [
                    "EFFORT is ignored for plain: it does not list the effort feature",
                    "EFFORT is ignored for plain-too: it does not list the effort feature",
                ],
                "{warnings:?}"
            );
        }

        #[test]
        fn a_saved_alias_removed_from_the_registry_is_reported() {
            let root = make_temp_dir("model_removed");
            let mut service = open(root.path(), &KEYS);
            service.switch_model("ds-flash").unwrap();
            drop(service);

            let service = AgentService::open(
                root.path().to_path_buf(),
                SessionStart::New,
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
        fn user_agent_types_load_at_startup_and_survive_a_model_switch() {
            let root = make_temp_dir("agent_types");
            let agents = make_temp_dir("agent_types_dir");
            std::fs::write(
                agents.path().join("scout.md"),
                "+++\nname = \"scout\"\ndescription = \"Finds things\"\n+++\nLook.",
            )
            .unwrap();
            std::fs::write(
                agents.path().join("cheap.md"),
                "+++\nname = \"cheap\"\ndescription = \"d\"\nmodel = \"ds-flash\"\n+++\nLook.",
            )
            .unwrap();
            let dir = agents.path().to_string_lossy().into_owned();
            let mut service = open(
                root.path(),
                &[("ANTHROPIC_API_KEY", "a"), ("SOCIUM_AGENTS_DIR", &dir)],
            );
            let agent_types = |service: &AgentService| {
                let agent = service
                    .tools
                    .iter()
                    .find(|tool| tool.name() == crate::constants::AGENT_TOOL_NAME)
                    .unwrap();
                agent.input_schema["properties"]["agent_type"]["enum"].clone()
            };

            assert_eq!(
                agent_types(&service),
                json!(["explorer", "worker", "scout"])
            );
            // Only models whose key is set can be chosen for a subagent.
            let agent = service
                .tools
                .iter()
                .find(|tool| tool.name() == crate::constants::AGENT_TOOL_NAME)
                .unwrap();
            assert_eq!(
                agent.input_schema["properties"]["model"]["enum"],
                json!(["opus", "sonnet", "haiku"])
            );
            let warnings = service.startup_warnings();
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].contains("cheap.md"), "{warnings:?}");
            assert!(warnings[0].contains("DEEPSEEK_API_KEY"), "{warnings:?}");

            service.switch_model("sonnet").unwrap();
            assert_eq!(
                agent_types(&service),
                json!(["explorer", "worker", "scout"])
            );
        }

        #[test]
        fn the_last_candidate_failing_is_a_startup_error() {
            let root = make_temp_dir("model_no_key");
            let err = AgentService::open(
                root.path().to_path_buf(),
                SessionStart::New,
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
            let names = |service: &AgentService| {
                service
                    .tools
                    .iter()
                    .map(|t| t.name().to_string())
                    .collect::<Vec<_>>()
            };
            let before = names(&service);
            service.last_measured_usage = Some(TokenUsage::default());

            service.switch_model("ds-flash").unwrap();
            assert_eq!(service.max_tokens, 32_000);
            // Core offers every model the same tools; a provider's built-in
            // form, such as Anthropic's text editor, is its adapter's job.
            assert_eq!(names(&service), before);
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
