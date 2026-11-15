pub(crate) mod request_options;
mod stream;

use crate::agent::types::AgentMessageRequest;
use crate::models::{AuthScheme, ModelConfig, ModelFeatures, ProviderConfig};
use crate::providers::error::{ProviderError, api_error};
use crate::providers::sse::SseParser;
use crate::providers::{
    Provider, ProviderCapabilities, ProviderEvent, ProviderKind, ProviderResponse,
};
use request_options::RequestOptions;
use serde_json::Value;
use std::io::{Error, ErrorKind};
use stream::StreamingMessageBuilder;

pub(crate) struct AnthropicProvider {
    name: String,
    /// The model this provider was built for.
    model: String,
    /// Anthropic's own API, where any Claude model's thinking can be sent
    /// back: the API drops what the current model cannot read, unbilled.
    first_party: bool,
    messages_url: String,
    auth: AuthScheme,
    api_key: String,
    anthropic_version: String,
    features: ModelFeatures,
    options: RequestOptions,
}

impl AnthropicProvider {
    pub(crate) fn from_config(
        provider: &ProviderConfig,
        model: &ModelConfig,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ProviderError> {
        let api_key = lookup(&provider.api_key_env)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::NotFound,
                    format!("{} is not set", provider.api_key_env),
                )
            })?;
        let anthropic_version =
            lookup("ANTHROPIC_VERSION").unwrap_or_else(|| "2023-06-01".to_string());
        let mut options = RequestOptions::from_lookup(lookup)?;
        options.warn_unsupported(&model.alias, model.features);
        Ok(Self {
            name: provider.name.clone(),
            model: model.id.clone(),
            first_party: provider.base_url == crate::models::ANTHROPIC_BASE_URL,
            messages_url: format!("{}/v1/messages", provider.base_url),
            auth: provider.auth,
            api_key,
            anthropic_version,
            features: model.features,
            options,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_credentials(
        api_key: impl Into<String>,
        anthropic_version: impl Into<String>,
    ) -> Self {
        Self {
            name: "anthropic".into(),
            model: String::new(),
            first_party: true,
            messages_url: "https://api.anthropic.com/v1/messages".into(),
            auth: AuthScheme::ApiKeyHeader,
            api_key: api_key.into(),
            anthropic_version: anthropic_version.into(),
            features: ModelFeatures::ALL,
            options: RequestOptions::default(),
        }
    }

    /// The HTTP request for `request`, without sending it.
    fn build_request(
        &self,
        client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let mut body = serde_json::to_value(request)?;
        body["stream"] = Value::Bool(true);
        self.options.apply(&mut body, self.features);
        let (header, value) = match self.auth {
            AuthScheme::ApiKeyHeader => ("x-api-key", self.api_key.clone()),
            AuthScheme::Bearer => ("authorization", format!("Bearer {}", self.api_key)),
        };
        let mut key = reqwest::header::HeaderValue::from_str(&value)
            .map_err(|_| ProviderError::Api("Invalid API key header".into()))?;
        key.set_sensitive(true);
        let mut http_request = client
            .post(&self.messages_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(header, key)
            .header("anthropic-version", &self.anthropic_version);
        if let Some(beta) = self.options.beta_header(self.features) {
            http_request = http_request.header("anthropic-beta", beta);
        }
        Ok(http_request.json(&body))
    }
}

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn label(&self) -> String {
        self.name.clone()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_tools: true,
        }
    }

    fn startup_warnings(&self) -> Vec<String> {
        self.options.warnings.clone()
    }

    /// Anthropic's API takes thinking from any Claude model it served. A
    /// compatible endpoint is only known to accept its own model's.
    fn reads_thinking_from(&self, origin: Option<&crate::agent::types::Origin>) -> bool {
        origin.is_some_and(|origin| {
            origin.provider == self.name && (self.first_party || origin.model == self.model)
        })
    }

    async fn send_message(
        &self,
        client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
        on_delta: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<ProviderResponse, ProviderError> {
        let mut response = self
            .build_request(client, request)?
            .send()
            .await
            .map_err(crate::providers::error::transport)?;
        if !response.status().is_success() {
            return Err(api_error(&self.name, response).await);
        }

        let mut parser = SseParser::default();
        let mut builder = StreamingMessageBuilder::default();
        let mut forward = |event: ProviderEvent| on_delta(event);

        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(crate::providers::error::transport)?
        {
            for event in parser.push(&chunk)? {
                forward(ProviderEvent::Raw {
                    json: stream::displayable_raw_event(&event),
                });
                builder.apply_event(&event, &mut forward)?;
            }
        }

        if let Some(event) = parser.finish()? {
            forward(ProviderEvent::Raw {
                json: stream::displayable_raw_event(&event),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::Message;
    use crate::models::ModelRegistry;
    use crate::tools::tools_for_model;

    fn built_request(registry: &ModelRegistry, alias: &str) -> reqwest::Request {
        let model = registry.find(alias).unwrap();
        let provider = AnthropicProvider::from_config(
            registry.provider(&model.provider).unwrap(),
            model,
            &|key: &str| (key.ends_with("_API_KEY")).then(|| "secret-key".to_string()),
        )
        .unwrap();
        let messages = [Message::user_text("hi".into())];
        let tools = tools_for_model(model.features.anthropic_text_editor);
        let request = AgentMessageRequest::new(&model.id, 100, &messages, &tools);
        provider
            .build_request(&reqwest::Client::new(), &request)
            .unwrap()
            .build()
            .unwrap()
    }

    fn json_body(request: &reqwest::Request) -> Value {
        serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap()
    }

    #[test]
    fn builtin_models_send_anthropic_extensions_with_an_api_key_header() {
        let request = built_request(&ModelRegistry::builtin(), "opus");
        assert_eq!(
            request.url().as_str(),
            "https://api.anthropic.com/v1/messages"
        );
        let key = &request.headers()["x-api-key"];
        assert_eq!(key, "secret-key");
        assert!(key.is_sensitive());
        assert!(request.headers().get("authorization").is_none());
        let body = json_body(&request);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(
            body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["type"] == "text_editor_20250728")
        );
    }

    #[test]
    fn compatible_endpoints_get_a_bearer_key_and_no_anthropic_extensions() {
        let registry = ModelRegistry::from_toml(
            r#"
[providers.deepseek]
wire = "anthropic"
base_url = "https://api.deepseek.com/anthropic"
api_key_env = "DEEPSEEK_API_KEY"
auth = "bearer"

[[models]]
alias = "ds-flash"
provider = "deepseek"
id = "deepseek-flash"
"#,
        )
        .unwrap();
        let request = built_request(&registry, "ds-flash");
        assert_eq!(
            request.url().as_str(),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
        let auth = &request.headers()["authorization"];
        assert_eq!(auth, "Bearer secret-key");
        assert!(auth.is_sensitive());
        assert!(request.headers().get("x-api-key").is_none());
        assert!(request.headers().get("anthropic-beta").is_none());
        let body = json_body(&request);
        assert_eq!(body["model"], "deepseek-flash");
        assert!(body.get("thinking").is_none());
        assert!(body.get("output_config").is_none());
        for tool in body["tools"].as_array().unwrap() {
            assert!(tool.get("type").is_none(), "{tool}");
            assert!(tool.get("eager_input_streaming").is_none(), "{tool}");
            assert!(tool["input_schema"].is_object(), "{tool}");
        }
    }

    #[test]
    fn a_missing_key_names_the_variable() {
        let registry = ModelRegistry::builtin();
        let model = registry.find("opus").unwrap();
        let err = AnthropicProvider::from_config(
            registry.provider("anthropic").unwrap(),
            model,
            &|_: &str| None,
        )
        .err()
        .unwrap();
        assert_eq!(err.to_string(), "ANTHROPIC_API_KEY is not set");
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;
    use crate::agent::types::Message;
    use crate::providers::test_server::serve_paced;
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn provider(base_url: &str) -> AnthropicProvider {
        let registry = crate::models::ModelRegistry::from_toml(&format!(
            "[providers.local]\nwire = \"anthropic\"\nbase_url = \"{base_url}\"\napi_key_env = \"LOCAL_KEY\"\n[[models]]\nalias = \"m\"\nprovider = \"local\"\nid = \"local-model\""
        ))
        .unwrap();
        AnthropicProvider::from_config(
            registry.provider("local").unwrap(),
            registry.find("m").unwrap(),
            &|key: &str| (key == "LOCAL_KEY").then(|| "k".to_string()),
        )
        .unwrap()
    }

    fn event(value: serde_json::Value) -> String {
        format!("data: {value}\n\n")
    }

    fn reply(delays: &[Duration]) -> Vec<(Duration, String)> {
        let mut events = vec![
            (
                Duration::ZERO,
                event(json!({"type":"message_start","message":{"usage":{"input_tokens":1}}})),
            ),
            (
                Duration::ZERO,
                event(
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                ),
            ),
        ];
        for delay in delays {
            events.push((
                *delay,
                event(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}})),
            ));
        }
        for value in [
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
            json!({"type":"message_stop"}),
        ] {
            events.push((Duration::ZERO, event(value)));
        }
        events
    }

    async fn send(
        base_url: &str,
        client: &reqwest::Client,
    ) -> Result<ProviderResponse, ProviderError> {
        let messages = [Message::user_text("hi".into())];
        let request = AgentMessageRequest::new("local-model", 100, &messages, &[]);
        provider(base_url)
            .send_message(client, &request, &mut |_| {})
            .await
    }

    #[tokio::test]
    async fn a_reply_that_keeps_streaming_outlasts_the_idle_limit() {
        // Six tokens 300 ms apart: 1.8 s in all, past a 1 s idle limit.
        let base_url = serve_paced(reply(&[Duration::from_millis(300); 6])).await;
        let client = crate::http_client(Duration::from_secs(5), Duration::from_secs(1)).unwrap();
        let response = send(&base_url, &client).await.unwrap();
        assert_eq!(
            response.message.content,
            [crate::agent::types::ContentBlock::Text {
                text: "xxxxxx".into()
            }]
        );
    }

    #[tokio::test]
    async fn a_stalled_reply_fails_with_a_plain_message() {
        let base_url = serve_paced(reply(&[Duration::ZERO, Duration::from_secs(3)])).await;
        let client = crate::http_client(Duration::from_secs(5), Duration::from_secs(1)).unwrap();
        let started = Instant::now();
        let err = send(&base_url, &client).await.err().unwrap();
        assert!(started.elapsed() < Duration::from_millis(2500));
        assert!(err.is_retryable());
        assert!(
            err.to_string().starts_with("No data from the provider"),
            "{err}"
        );
    }
}
