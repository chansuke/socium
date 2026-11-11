pub mod anthropic;
pub mod error;

use crate::agent::types::{AgentMessageRequest, AgentMessageResponse};
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::error::ProviderError;
use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;
use std::io::{self, Error, ErrorKind};

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

pub(crate) enum DeltaEvent {
    Text(String),
    Thinking(String),
}

impl ProviderKind {
    pub fn from_env() -> io::Result<Self> {
        let raw = env::var("PROVIDER").unwrap_or_else(|_| "anthropic".to_string());
        Self::parse(&raw)
    }

    fn parse(raw: &str) -> io::Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            other => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("Unsupported provider '{other}'. Currently supported: anthropic"),
            )),
        }
    }
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
    fn capabilities(&self) -> ProviderCapabilities;
    async fn send_message(
        &self,
        client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
        on_delta: &mut (dyn FnMut(DeltaEvent) + Send),
    ) -> Result<ProviderResponse, ProviderError>;
}

pub(crate) fn provider_from_env() -> Result<Box<dyn Provider>, ProviderError> {
    match ProviderKind::from_env()? {
        ProviderKind::Anthropic => Ok(Box::new(AnthropicProvider::from_env()?)),
    }
}
