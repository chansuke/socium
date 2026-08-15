pub mod agent;
pub mod constants;
pub mod error;
pub mod providers;
pub mod tools;

pub use crate::agent::context::ContextMetrics;
pub use crate::error::AgentError;

use crate::agent::compaction::{
    CompactionRequest, compaction_threshold_tokens, retention_boundary, summarize_with_llm,
};
use crate::agent::context::{
    context_metrics_for_input, prepare_context_messages, resolve_context_budget, summarize_messages,
};
use crate::agent::state::{
    ConversationState, ConversationStateRef, ConversationSummary, ConversationSummaryState,
    load_json_file, save_json_file,
};
use crate::agent::turn::{TurnContext, extract_text, run_agent_turn};
use crate::agent::types::Message;
use crate::constants::*;
use crate::providers::error::ProviderError;
use crate::providers::{Provider, TokenUsage, provider_from_env};
use crate::tools::{ToolDefinition, default_tools};
use std::env;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::time::Duration;

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
    ApprovalRequired {
        tool: String,
        reason: String,
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
    workspace_root: PathBuf,
    conversation_path: PathBuf,
    summary_path: PathBuf,
    messages: Vec<Message>,
    summary_state: ConversationSummaryState,
    provider: Box<dyn Provider>,
    model: String,
    max_tokens: u32,
    tools: Vec<ToolDefinition>,
    client: reqwest::Client,
    /// Usage measured on the most recent model call. Volatile: not
    /// persisted, so the first turn after a restart falls back to the
    /// heuristic estimate.
    last_measured_usage: Option<TokenUsage>,
}

impl AgentService {
    pub fn from_current_dir() -> Result<Self, AgentError> {
        Self::from_env(env::current_dir().map_err(AgentError::State)?)
    }

    /// Construction seam that takes the provider and HTTP client directly,
    /// so tests can inject a mock `Provider` without touching the
    /// environment or the network.
    pub(crate) fn with_provider(
        workspace_root: PathBuf,
        provider: Box<dyn Provider>,
        model: String,
        max_tokens: u32,
        client: reqwest::Client,
    ) -> Self {
        let conversation_path = workspace_root
            .join(STATE_DIR_NAME)
            .join(CONVERSATION_STATE_FILE_NAME);
        let summary_path = workspace_root
            .join(STATE_DIR_NAME)
            .join(SUMMARY_STATE_FILE_NAME);
        Self {
            workspace_root,
            conversation_path,
            summary_path,
            messages: Vec::new(),
            summary_state: ConversationSummaryState::default(),
            provider,
            model,
            max_tokens,
            tools: default_tools(),
            client,
            last_measured_usage: None,
        }
    }

    pub fn from_env(workspace_root: PathBuf) -> Result<Self, AgentError> {
        let conversation_path = workspace_root
            .join(STATE_DIR_NAME)
            .join(CONVERSATION_STATE_FILE_NAME);
        let summary_path = workspace_root
            .join(STATE_DIR_NAME)
            .join(SUMMARY_STATE_FILE_NAME);

        let messages = match load_json_file::<ConversationState>(&conversation_path) {
            Ok(state) => {
                let mut messages = state.messages;
                if messages.len() > MAX_PERSISTED_MESSAGES {
                    let overflow = messages.len() - MAX_PERSISTED_MESSAGES;
                    messages.drain(0..overflow);
                    eprintln!(
                        "Persisted conversation exceeded {} messages and was truncated.",
                        MAX_PERSISTED_MESSAGES
                    );
                }
                messages
            }
            Err(err) => {
                eprintln!(
                    "Failed to load persisted conversation '{}': {err}. Starting with an empty conversation.",
                    conversation_path.display()
                );
                Vec::new()
            }
        };
        let mut summary_state = match load_json_file::<ConversationSummaryState>(&summary_path) {
            Ok(state) => state,
            Err(err) => {
                eprintln!(
                    "Failed to load persisted summaries '{}': {err}. Continuing without summaries.",
                    summary_path.display()
                );
                ConversationSummaryState::default()
            }
        };
        if summary_state
            .summary
            .as_ref()
            .is_some_and(|summary| summary.covered_messages > messages.len())
        {
            summary_state = ConversationSummaryState::default();
        }

        let provider = provider_from_env()?;
        let model = env::var("MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string());
        let max_tokens = env::var("MAX_TOKENS")
            .unwrap_or_else(|_| "1000".to_string())
            .parse::<u32>()
            .map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("MAX_TOKENS must be an unsigned integer: {e}"),
                )
            })?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .build()
            .map_err(ProviderError::Http)?;

        let mut service = Self::with_provider(workspace_root, provider, model, max_tokens, client);
        service.messages = messages;
        service.summary_state = summary_state;
        Ok(service)
    }

    pub fn clear(&mut self) -> std::io::Result<()> {
        self.messages.clear();
        self.summary_state = ConversationSummaryState::default();
        self.persist_state()
    }

    pub fn transcript_entries(&self) -> Vec<TranscriptEntry> {
        self.messages
            .iter()
            .filter_map(|message| {
                let text = match &message.content {
                    crate::agent::types::MessageContent::Text(text) => text.clone(),
                    crate::agent::types::MessageContent::Blocks(blocks) => extract_text(blocks),
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
        self.messages.push(Message::user_text(user_input));
        if self.should_auto_compact()? {
            on_progress(AgentProgress::CompactionStarted);
            match self.run_compaction().await {
                Ok(report) => on_progress(AgentProgress::CompactionFinished {
                    covered_messages: report.covered_messages,
                    used_llm: report.used_llm,
                }),
                // Compaction must never block the conversation; the
                // budget-based trimming below still bounds the prompt.
                Err(AgentError::NothingToCompact) => {}
                Err(err) => eprintln!("Auto-compaction failed: {err}"),
            }
        }
        let prepared_context = prepare_context_messages(
            &self.messages,
            &self.summary_state,
            self.provider.kind(),
            &self.model,
            self.max_tokens,
            &self.tools,
        )?;
        let initial_len = prepared_context.messages.len();
        let mut context_messages = prepared_context.messages;
        let turn_context = TurnContext {
            provider: self.provider.as_ref(),
            client: &self.client,
            model: &self.model,
            max_tokens: self.max_tokens,
            tools: &self.tools,
            workspace_root: &self.workspace_root,
        };
        let outcome =
            run_agent_turn(&turn_context, &mut context_messages, &mut on_progress).await?;
        if outcome.last_usage.is_some() {
            self.last_measured_usage = outcome.last_usage;
        }
        let new_messages = context_messages.split_off(initial_len);
        self.messages.extend(new_messages);
        self.summary_state = prepared_context.summary_state;
        self.persist_state()?;
        Ok(AgentReply {
            text: outcome.assistant_text_chunks.join("\n"),
        })
    }

    pub fn context_metrics(&self, pending_user_input: &str) -> std::io::Result<ContextMetrics> {
        let mut metrics = context_metrics_for_input(
            &self.messages,
            pending_user_input,
            &self.summary_state,
            self.provider.kind(),
            &self.model,
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
        self.run_compaction().await
    }

    fn should_auto_compact(&self) -> Result<bool, AgentError> {
        let Some(usage) = self.last_measured_usage else {
            return Ok(false);
        };
        let budget = resolve_context_budget(self.provider.kind(), &self.model, self.max_tokens)?;
        let threshold = compaction_threshold_tokens(budget.context_window_tokens);
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
        let (text, used_llm) =
            match summarize_with_llm(self.provider.as_ref(), &self.client, &compaction_request)
                .await
            {
                Ok(text) => (text, true),
                Err(err) => {
                    eprintln!(
                        "LLM compaction failed, falling back to deterministic summary: {err}"
                    );
                    (summarize_messages(&self.messages[..boundary]), false)
                }
            };

        self.summary_state = ConversationSummaryState {
            summary: Some(ConversationSummary {
                covered_messages: boundary,
                text,
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
                messages: &self.messages,
            },
        )?;
        save_json_file(&self.summary_path, &self.summary_state)
    }
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use crate::agent::state::ConversationSummaryState;
    use crate::agent::types::{ContentBlock, Message, MessageContent, Role};
    use crate::constants::{CONVERSATION_STATE_FILE_NAME, STATE_DIR_NAME, SUMMARY_STATE_FILE_NAME};
    use crate::providers::TokenUsage;
    use crate::providers::anthropic::AnthropicProvider;
    use crate::test_support::{ScriptedProvider, make_temp_dir, text_provider_response};
    use crate::tools::default_tools;
    use crate::{AgentService, TranscriptEntry, TranscriptRole};
    use serde_json::json;

    #[test]
    fn transcript_entries_include_text_messages_and_skip_tool_only_blocks() {
        let workspace = make_temp_dir("transcript");
        let conversation_path = workspace
            .path()
            .join(STATE_DIR_NAME)
            .join(CONVERSATION_STATE_FILE_NAME);
        let summary_path = workspace
            .path()
            .join(STATE_DIR_NAME)
            .join(SUMMARY_STATE_FILE_NAME);
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

        let service = AgentService {
            workspace_root: workspace.path().to_path_buf(),
            conversation_path,
            summary_path,
            messages,
            summary_state: ConversationSummaryState::default(),
            provider: Box::new(AnthropicProvider::with_credentials("test", "2023-06-01")),
            model: "test-model".to_string(),
            max_tokens: 128,
            tools: default_tools(),
            client: reqwest::Client::builder().build().unwrap(),
            last_measured_usage: None,
        };

        assert_eq!(
            service.transcript_entries(),
            vec![
                TranscriptEntry {
                    role: TranscriptRole::User,
                    text: "hello".to_string(),
                },
                TranscriptEntry {
                    role: TranscriptRole::Assistant,
                    text: "answer".to_string(),
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

        let persisted: ConversationSummaryState = crate::agent::state::load_json_file(
            &workspace
                .path()
                .join(STATE_DIR_NAME)
                .join(SUMMARY_STATE_FILE_NAME),
        )
        .unwrap();
        assert_eq!(persisted.summary.unwrap().covered_messages, 15);
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
}
