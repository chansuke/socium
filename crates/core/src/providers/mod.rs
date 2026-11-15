pub mod anthropic;
pub mod error;
pub(crate) mod openai_chat;
pub(crate) mod openai_responses;
pub(crate) mod sse;
#[cfg(test)]
pub(crate) mod test_server;

use crate::agent::types::{AgentMessageRequest, AgentMessageResponse};
use crate::models::{ModelConfig, ProviderConfig, Wire};
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::error::ProviderError;
use crate::providers::openai_chat::OpenAiChatProvider;
use crate::providers::openai_responses::OpenAiResponsesProvider;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Token counts reported by the provider for a single model call.
/// Mirrors the Anthropic `usage` payload; providers that do not report
/// usage leave it absent on the response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
}

impl TokenUsage {
    /// Prompt-side total as the API meters it: live input plus cache
    /// writes and cache reads.
    pub fn total_input_tokens(&self) -> u32 {
        self.input_tokens
            .saturating_add(self.cache_creation_input_tokens)
            .saturating_add(self.cache_read_input_tokens)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub supports_tools: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    OpenAiChat,
    OpenAiResponses,
}

pub(crate) struct ProviderResponse {
    pub message: AgentMessageResponse,
    pub streamed_text: bool,
    /// Measured usage for this call, when the provider reports it.
    pub usage: Option<TokenUsage>,
}

/// What a streamed content block holds, known when the block opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlockKind {
    Text,
    Thinking,
    ToolUse { id: String, name: String },
}

/// Incremental view of a streaming response. Text and thinking deltas are
/// what the UI shows; tool input deltas are display-only, and the complete,
/// validated input still arrives in the final response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderEvent {
    BlockStarted {
        index: usize,
        kind: BlockKind,
    },
    TextDelta {
        index: usize,
        text: String,
    },
    ThinkingDelta {
        index: usize,
        text: String,
    },
    ToolInputDelta {
        index: usize,
        partial_json: String,
    },
    BlockFinished {
        index: usize,
    },
    MessageFinished {
        stop_reason: String,
        usage: Option<TokenUsage>,
    },
    /// One provider stream event exactly as received, for inspection.
    Raw {
        json: String,
    },
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Anthropic => "anthropic",
            Self::OpenAiChat => "openai-chat",
            Self::OpenAiResponses => "openai-responses",
        };
        write!(f, "{name}")
    }
}

/// A model provider: identity, capabilities, and streaming message
/// transport behind one dyn-compatible interface.
#[async_trait::async_trait]
pub(crate) trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    /// Name shown in progress and errors: the registry provider name.
    fn label(&self) -> String {
        self.kind().to_string()
    }
    fn capabilities(&self) -> ProviderCapabilities;
    /// Configuration problems that were tolerated at construction and
    /// should be shown to the user once.
    fn startup_warnings(&self) -> Vec<String> {
        Vec::new()
    }
    async fn send_message(
        &self,
        client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
        on_delta: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<ProviderResponse, ProviderError>;
}

/// Builds the provider for `model`, reading its API key through `lookup`
/// so a missing key is reported before the first request.
pub(crate) fn build_provider(
    provider: &ProviderConfig,
    model: &ModelConfig,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Box<dyn Provider>, ProviderError> {
    match provider.wire {
        Wire::Anthropic => Ok(Box::new(AnthropicProvider::from_config(
            provider, model, lookup,
        )?)),
        Wire::OpenAiChat => Ok(Box::new(OpenAiChatProvider::from_config(
            provider, model, lookup,
        )?)),
        Wire::OpenAiResponses => Ok(Box::new(OpenAiResponsesProvider::from_config(
            provider, model, lookup,
        )?)),
    }
}

/// Whether a response with this stop reason may be used. A reply cut at
/// the output limit is kept when it calls no tool, since its text is
/// complete as far as it goes; a tool call cut there may be truncated.
pub(crate) fn stop_reason_allows_release(
    stop_reason: Option<&str>,
    has_tools: bool,
) -> Result<(), ProviderError> {
    match stop_reason {
        Some("tool_use") if has_tools => Ok(()),
        Some("end_turn" | "stop_sequence") if !has_tools => Ok(()),
        Some("max_tokens") if !has_tools => Ok(()),
        Some("max_tokens") => Err(ProviderError::Stream(
            "Response stopped at the output token limit (stop reason: max_tokens) while writing a tool call; \
             raise MAX_TOKENS or the model's max_output_tokens"
                .into(),
        )),
        Some("refusal") => Err(ProviderError::Stream(
            "The model declined to continue (stop reason: refusal)".into(),
        )),
        Some("model_context_window_exceeded") => Err(ProviderError::Stream(
            "The request filled the model's context window; run /compact and continue".into(),
        )),
        Some(reason) => Err(ProviderError::Stream(format!(
            "Response stopped without safe completion (stop reason: {reason})"
        ))),
        None => Err(ProviderError::Stream(
            "Response ended without a stop reason".into(),
        )),
    }
}

/// `Authorization: Bearer <key>`, marked sensitive so it is never logged.
pub(crate) fn bearer(api_key: &str) -> Result<reqwest::header::HeaderValue, ProviderError> {
    let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_key}"))
        .map_err(|_| ProviderError::Api("Invalid API key header".into()))?;
    value.set_sensitive(true);
    Ok(value)
}
