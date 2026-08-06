pub mod anthropic;
pub mod codex;
pub mod error;
pub mod opencode;

use crate::agent::types::{AgentMessageRequest, AgentMessageResponse};
use crate::providers::anthropic::AnthropicProvider;
use crate::providers::codex::CodexProvider;
use crate::providers::error::ProviderError;
use crate::providers::opencode::OpencodeProvider;
use std::env;
use std::fmt;
use std::io::{self, Error, ErrorKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub supports_tools: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    Codex,
    Opencode,
}

pub(crate) struct ProviderResponse {
    pub message: AgentMessageResponse,
    pub streamed_text: bool,
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
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::Opencode),
            other => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("Unsupported provider '{other}'. Valid values: anthropic, codex, opencode"),
            )),
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Anthropic => "anthropic",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
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
        ProviderKind::Codex => Ok(Box::new(CodexProvider)),
        ProviderKind::Opencode => Ok(Box::new(OpencodeProvider)),
    }
}
