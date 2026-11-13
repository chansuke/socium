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

    async fn send_message(
        &self,
        client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
        on_delta: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<ProviderResponse, ProviderError> {
        let mut response = self.build_request(client, request)?.send().await?;
        if !response.status().is_success() {
            return Err(api_error(&self.name, response).await);
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
