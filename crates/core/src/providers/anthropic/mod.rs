mod request_options;
mod sse;
mod stream;

use crate::agent::types::AgentMessageRequest;
use crate::constants::ANTHROPIC_MESSAGES_API;
use crate::providers::error::ProviderError;
use crate::providers::{
    Provider, ProviderCapabilities, ProviderEvent, ProviderKind, ProviderResponse,
};
use request_options::RequestOptions;
use serde_json::Value;
use sse::SseParser;
use std::env;
use std::io::{Error, ErrorKind};
use stream::StreamingMessageBuilder;

const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;
const MAX_ERROR_MESSAGE_CHARS: usize = 300;

pub(crate) struct AnthropicProvider {
    api_key: String,
    anthropic_version: String,
    options: RequestOptions,
}

impl AnthropicProvider {
    pub(crate) fn from_env() -> Result<Self, ProviderError> {
        let api_key = env::var("ANTHROPIC_API_KEY")
            .map_err(|_| Error::new(ErrorKind::NotFound, "ANTHROPIC_API_KEY is not set"))?;
        let anthropic_version =
            env::var("ANTHROPIC_VERSION").unwrap_or_else(|_| "2023-06-01".to_string());
        let options = RequestOptions::from_lookup(|key| env::var(key).ok())?;
        Ok(Self {
            api_key,
            anthropic_version,
            options,
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
            options: RequestOptions::default(),
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

    fn startup_warnings(&self) -> Vec<String> {
        self.options.warnings.clone()
    }

    async fn send_message(
        &self,
        client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
        on_delta: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<ProviderResponse, ProviderError> {
        let mut body = serde_json::to_value(request)?;
        body["stream"] = Value::Bool(true);
        self.options.apply(&mut body);
        let mut key = reqwest::header::HeaderValue::from_str(&self.api_key)
            .map_err(|_| ProviderError::Api("Invalid API key header".into()))?;
        key.set_sensitive(true);
        let mut http_request = client
            .post(ANTHROPIC_MESSAGES_API)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("x-api-key", key)
            .header("anthropic-version", &self.anthropic_version);
        if let Some(beta) = self.options.beta_header() {
            http_request = http_request.header("anthropic-beta", beta);
        }
        let mut response = http_request.json(&body).send().await?;
        let status = response.status();
        if !status.is_success() {
            let mut body = Vec::new();
            while body.len() < MAX_ERROR_BODY_BYTES
                && let Ok(Some(chunk)) = response.chunk().await
            {
                body.extend_from_slice(&chunk);
            }
            body.truncate(MAX_ERROR_BODY_BYTES);
            return Err(ProviderError::Api(api_error_message(status, &body)));
        }

        let mut parser = SseParser::default();
        let mut builder = StreamingMessageBuilder::default();
        let mut forward = |event: ProviderEvent| on_delta(event);

        while let Some(chunk) = response.chunk().await? {
            for event in parser.push(&chunk)? {
                forward(ProviderEvent::Raw {
                    json: event.clone(),
                });
                builder.apply_event(&event, &mut forward)?;
            }
        }

        if let Some(event) = parser.finish()? {
            forward(ProviderEvent::Raw {
                json: event.clone(),
            });
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

/// Formats a non-2xx response for the UI and persisted transcript. Only the
/// documented `error.type` and `error.message` fields are shown, bounded and
/// stripped of control characters, so the raw body (which may echo request
/// text) and terminal escape sequences never reach the screen.
fn api_error_message(status: reqwest::StatusCode, body: &[u8]) -> String {
    let fallback = format!("Anthropic API error {status}");
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return fallback;
    };
    let (Some(kind), Some(message)) = (
        value["error"]["type"].as_str(),
        value["error"]["message"].as_str(),
    ) else {
        return fallback;
    };
    let mut cleaned = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c });
    let mut shown = cleaned
        .by_ref()
        .take(MAX_ERROR_MESSAGE_CHARS)
        .collect::<String>();
    if cleaned.next().is_some() {
        shown.push('…');
    }
    let kind = kind.chars().filter(|c| !c.is_control()).collect::<String>();
    format!("{fallback}: {kind}: {shown}")
}

#[cfg(test)]
mod tests {
    use super::api_error_message;
    use reqwest::StatusCode;

    #[test]
    fn api_error_shows_type_and_message() {
        let body = br#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."},"request_id":"req_1"}"#;
        assert_eq!(
            api_error_message(StatusCode::BAD_REQUEST, body),
            "Anthropic API error 400 Bad Request: invalid_request_error: Your credit balance is too low to access the Anthropic API."
        );
    }

    #[test]
    fn api_error_falls_back_to_status_for_unrecognized_bodies() {
        for body in [
            &b"<html>bad gateway</html>"[..],
            b"",
            br#"{"error":{"type":"x""#,
        ] {
            assert_eq!(
                api_error_message(StatusCode::BAD_GATEWAY, body),
                "Anthropic API error 502 Bad Gateway"
            );
        }
    }

    #[test]
    fn api_error_message_is_bounded_and_strips_control_characters() {
        let message = format!("bad\u{1b}[31m input\n{}", "x".repeat(1000));
        let body =
            serde_json::json!({"error": {"type": "invalid_request_error", "message": message}});
        let shown = api_error_message(StatusCode::BAD_REQUEST, body.to_string().as_bytes());
        assert!(!shown.chars().any(char::is_control));
        assert!(shown.contains("bad [31m input x"));
        assert!(shown.ends_with('…'));
        assert!(shown.chars().count() < 400);
    }
}
