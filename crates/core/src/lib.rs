pub mod agent;
pub mod constants;
pub mod error;
pub mod providers;
pub mod tools;

pub use crate::agent::context::ContextMetrics;
pub use crate::error::AgentError;

use crate::agent::context::{context_metrics_for_input, prepare_context_messages};
use crate::agent::state::{
    ConversationState, ConversationStateRef, ConversationSummaryState, load_json_file,
    save_json_file,
};
use crate::agent::turn::{TurnContext, extract_text, run_agent_turn};
use crate::agent::types::Message;
use crate::constants::*;
use crate::providers::error::ProviderError;
use crate::providers::{Provider, provider_from_env};
use crate::tools::{ToolDefinition, default_tools};
use std::env;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::time::Duration;

pub struct AgentReply {
    pub text: String,
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
        let assistant_chunks =
            run_agent_turn(&turn_context, &mut context_messages, &mut on_progress).await?;
        let new_messages = context_messages.split_off(initial_len);
        self.messages.extend(new_messages);
        self.summary_state = prepared_context.summary_state;
        self.persist_state()?;
        Ok(AgentReply {
            text: assistant_chunks.join("\n"),
        })
    }

    pub fn context_metrics(&self, pending_user_input: &str) -> std::io::Result<ContextMetrics> {
        context_metrics_for_input(
            &self.messages,
            pending_user_input,
            &self.summary_state,
            self.provider.kind(),
            &self.model,
            self.max_tokens,
            &self.tools,
        )
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
    use crate::providers::anthropic::AnthropicProvider;
    use crate::test_support::make_temp_dir;
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
}
