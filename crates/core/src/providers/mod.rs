pub mod anthropic;
pub mod error;

use crate::agent::types::{AgentMessageRequest, AgentMessageResponse};
use crate::models::{ModelConfig, ProviderConfig, Wire};
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::error::ProviderError;
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
    }
}
