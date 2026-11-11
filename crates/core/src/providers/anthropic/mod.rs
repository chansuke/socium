mod sse;
mod stream;

use crate::agent::types::AgentMessageRequest;
use crate::constants::ANTHROPIC_MESSAGES_API;
use crate::providers::error::ProviderError;
use crate::providers::{
    DeltaEvent, Provider, ProviderCapabilities, ProviderKind, ProviderResponse,
};
use serde_json::Value;
use sse::SseParser;
use std::env;
use std::io::{Error, ErrorKind};
use stream::StreamingMessageBuilder;

pub(crate) struct AnthropicProvider {
    api_key: String,
    anthropic_version: String,
}

impl AnthropicProvider {
    pub(crate) fn from_env() -> Result<Self, ProviderError> {
        let api_key = env::var("ANTHROPIC_API_KEY")
            .map_err(|_| Error::new(ErrorKind::NotFound, "ANTHROPIC_API_KEY is not set"))?;
        let anthropic_version =
            env::var("ANTHROPIC_VERSION").unwrap_or_else(|_| "2023-06-01".to_string());
        Ok(Self {
            api_key,
            anthropic_version,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_credentials(
        api_key: impl Into<String>,
        anthropic_version: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            anthropic_version: anthropic_version.into(),
        }
    }
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_tools: true,
        }
    }

    async fn send_message(
        &self,
        client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
        on_delta: &mut (dyn FnMut(DeltaEvent) + Send),
    ) -> Result<ProviderResponse, ProviderError> {
        let mut body = serde_json::to_value(request)?;
        body["stream"] = Value::Bool(true);
        if let Ok(budget_str) = std::env::var("THINKING_BUDGET_TOKENS")
            && let Ok(budget) = budget_str.parse::<u64>()
        {
            body["thinking"] = serde_json::json!({
                "type": "enabled",
                "budget_tokens": budget
            });
        }
        let mut key = reqwest::header::HeaderValue::from_str(&self.api_key)
            .map_err(|_| ProviderError::Api("Invalid API key header".into()))?;
        key.set_sensitive(true);
        let mut response = client
            .post(ANTHROPIC_MESSAGES_API)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("x-api-key", key)
            .header("anthropic-version", &self.anthropic_version)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            // Do not buffer arbitrary error bodies or reflect server-supplied
            // credentials/request text into the UI and persisted transcript.
            return Err(ProviderError::Api(format!("Anthropic API error {status}")));
        }

        let mut parser = SseParser::default();
        let mut builder = StreamingMessageBuilder::default();
        let mut forward = |event: DeltaEvent| on_delta(event);

        while let Some(chunk) = response.chunk().await? {
            for event in parser.push(&chunk)? {
                builder.apply_event(&event, &mut forward)?;
            }
        }

        if let Some(event) = parser.finish()? {
            builder.apply_event(&event, &mut forward)?;
        }

        let streamed_text = builder.streamed_text;
        let usage = builder.usage;
        let message = builder.finish()?;
        Ok(ProviderResponse {
            message,
            streamed_text,
            usage,
        })
    }
}
