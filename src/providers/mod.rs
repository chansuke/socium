pub mod anthropic;
pub mod codex;
pub mod opencode;

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
