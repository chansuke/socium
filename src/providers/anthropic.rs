use crate::constants::ANTHROPIC_MESSAGES_API;
use crate::{AgentMessageRequest, AgentMessageResponse};

use super::ProviderCapabilities;

pub const CAPABILITIES: ProviderCapabilities = ProviderCapabilities {
    supports_tools: true,
};

pub(crate) async fn send_message(
    client: &reqwest::Client,
    api_key: &str,
    anthropic_version: &str,
    message_request: &AgentMessageRequest<'_>,
) -> Result<AgentMessageResponse, Box<dyn std::error::Error>> {
    let response = client
        .post(ANTHROPIC_MESSAGES_API)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", anthropic_version)
        .json(message_request)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await?;
        return Err(
            std::io::Error::other(format!("Anthropic API error {status}: {error_body}")).into(),
        );
    }
    Ok(response.json().await?)
}
