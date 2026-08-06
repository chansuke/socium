use crate::agent::types::AgentMessageRequest;
use crate::providers::error::ProviderError;
use crate::providers::{
    DeltaEvent, Provider, ProviderCapabilities, ProviderKind, ProviderResponse,
};

pub(crate) struct OpencodeProvider;

#[async_trait::async_trait]
impl Provider for OpencodeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Opencode
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_tools: false,
        }
    }

    async fn send_message(
        &self,
        _client: &reqwest::Client,
        _request: &AgentMessageRequest<'_>,
        _on_delta: &mut (dyn FnMut(DeltaEvent) + Send),
    ) -> Result<ProviderResponse, ProviderError> {
        Err(ProviderError::NotImplemented("opencode"))
    }
}
