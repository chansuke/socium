pub mod anthropic;
pub mod codex;
pub mod opencode;

use crate::agent::types::{AgentMessageRequest, AgentMessageResponse};
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

#[derive(Debug, Clone)]
pub enum ProviderConfig {
    Anthropic {
        api_key: String,
        anthropic_version: String,
    },
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

    pub fn capabilities(self) -> ProviderCapabilities {
        match self {
            Self::Anthropic => anthropic::CAPABILITIES,
            Self::Codex => codex::CAPABILITIES,
            Self::Opencode => opencode::CAPABILITIES,
        }
    }

    pub fn config_from_env(self) -> io::Result<ProviderConfig> {
        match self {
            Self::Anthropic => {
                let api_key = env::var("ANTHROPIC_API_KEY")
                    .map_err(|_| Error::new(ErrorKind::NotFound, "ANTHROPIC_API_KEY is not set"))?;
                let anthropic_version =
                    env::var("ANTHROPIC_VERSION").unwrap_or_else(|_| "2023-06-01".to_string());
                Ok(ProviderConfig::Anthropic {
                    api_key,
                    anthropic_version,
                })
            }
            Self::Codex => Ok(ProviderConfig::Codex),
            Self::Opencode => Ok(ProviderConfig::Opencode),
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

pub(crate) async fn send_message<F>(
    provider: ProviderKind,
    client: &reqwest::Client,
    config: &ProviderConfig,
    message_request: &AgentMessageRequest<'_>,
    on_delta: &mut F,
) -> Result<ProviderResponse, Box<dyn std::error::Error>>
where
    F: FnMut(DeltaEvent) + Send,
{
    match (provider, config) {
        (
            ProviderKind::Anthropic,
            ProviderConfig::Anthropic {
                api_key,
                anthropic_version,
            },
        ) => {
            anthropic::send_message(
                client,
                api_key,
                anthropic_version,
                message_request,
                on_delta,
            )
            .await
        }
        (ProviderKind::Codex, ProviderConfig::Codex) => Err(Error::new(
            ErrorKind::Unsupported,
            "Provider 'codex' is not implemented yet",
        )
        .into()),
        (ProviderKind::Opencode, ProviderConfig::Opencode) => Err(Error::new(
            ErrorKind::Unsupported,
            "Provider 'opencode' is not implemented yet",
        )
        .into()),
        _ => Err(Error::new(
            ErrorKind::InvalidInput,
            "Provider kind does not match provider config",
        )
        .into()),
    }
}
