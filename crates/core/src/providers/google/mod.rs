//! Google's Gemini API (`wire = "google"`): `streamGenerateContent` with
//! server-sent events, as Google serves it and as OpenCode Zen relays it.
//! `base_url` includes the API version, such as
//! `https://generativelanguage.googleapis.com/v1beta`.
//!
//! Gemini returns thought signatures on response parts and validates the
//! ones on function calls in the current turn, so they are kept in the
//! history as thinking blocks and put back on the same parts
//! (https://aistudio.google.com/docs/thought-signatures?generate-content=true).

mod stream;

use crate::conversation::{AgentMessageRequest, ContentBlock, Message, MessageContent, Role};
use crate::models::{AuthScheme, ModelConfig, ModelFeatures, ProviderConfig};
use crate::providers::anthropic::request_options::RequestOptions;
use crate::providers::error::{ProviderError, api_error};
use crate::providers::sse::SseParser;
use crate::providers::{
    Provider, ProviderCapabilities, ProviderEvent, ProviderKind, ProviderResponse, bearer,
};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{Error, ErrorKind};
use stream::GoogleStreamBuilder;

/// Marks ids this wire made up for calls the API sent without one; they
/// are never sent back as Gemini ids.
const GENERATED_CALL_ID_PREFIX: &str = "gemini-call-";

/// The documented value that tells Gemini 3 and later to skip signature
/// validation for a function call it did not write, such as one from
/// another model earlier in the turn.
const SKIP_SIGNATURE: &str = "skip_thought_signature_validator";

pub(crate) struct GoogleProvider {
    name: String,
    model: String,
    stream_url: String,
    /// `None` for a local endpoint that takes no key.
    api_key: Option<String>,
    auth: AuthScheme,
    features: ModelFeatures,
    options: RequestOptions,
    /// `tools = false` in models.toml.
    supports_tools: bool,
}

impl GoogleProvider {
    pub(crate) fn from_config(
        provider: &ProviderConfig,
        model: &ModelConfig,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ProviderError> {
        let api_key = provider
            .api_key(lookup)
            .map_err(|message| Error::new(ErrorKind::NotFound, message))?;
        let mut options = RequestOptions::from_lookup(lookup)?;
        options.warn_unsupported(&model.alias, model.features);
        Ok(Self {
            name: provider.name.clone(),
            model: model.id.clone(),
            stream_url: format!(
                "{}/models/{}:streamGenerateContent?alt=sse",
                provider.base_url, model.id
            ),
            api_key,
            auth: provider.auth,
            features: model.features,
            options,
            supports_tools: model.tools,
        })
    }

    fn build_request(
        &self,
        client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let body = request_body(request, &self.name, self.features, &self.options);
        let mut http_request = client.post(&self.stream_url);
        if let Some(api_key) = &self.api_key {
            http_request = match self.auth {
                AuthScheme::Bearer => {
                    http_request.header(reqwest::header::AUTHORIZATION, bearer(api_key)?)
                }
                AuthScheme::GoogApiKey | AuthScheme::ApiKeyHeader => {
                    let mut key = reqwest::header::HeaderValue::from_str(api_key)
                        .map_err(|_| ProviderError::Api("Invalid API key header".into()))?;
                    key.set_sensitive(true);
                    http_request.header("x-goog-api-key", key)
                }
            };
        }
        Ok(http_request.json(&body))
    }
}

#[async_trait::async_trait]
impl Provider for GoogleProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Google
    }

    fn label(&self) -> String {
        self.name.clone()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_tools: self.supports_tools,
        }
    }

    fn startup_warnings(&self) -> Vec<String> {
        self.options.warnings.clone()
    }

    /// Thought signatures go back only to the model that wrote them.
    fn reads_thinking_from(&self, origin: Option<&crate::conversation::Origin>) -> bool {
        origin.is_some_and(|origin| origin.provider == self.name && origin.model == self.model)
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
        let mut builder = GoogleStreamBuilder::default();
        let mut forward = |event: ProviderEvent| on_delta(event);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(crate::providers::error::transport)?
        {
            for data in parser.push(&chunk)? {
                forward(ProviderEvent::Raw { json: data.clone() });
                builder.apply_event(&data, &mut forward)?;
            }
        }
        if let Some(data) = parser.finish()? {
            forward(ProviderEvent::Raw { json: data.clone() });
            builder.apply_event(&data, &mut forward)?;
        }
        builder.finish(&mut forward)
    }
}

/// A thought signature as a thinking block's signature. The JSON wrapper
/// keeps it apart from other wires' signatures.
fn encode_signature(signature: &str) -> String {
    json!({ "thoughtSignature": signature }).to_string()
}

/// The thought signature stored by [`encode_signature`], if `signature`
/// holds one.
fn decode_signature(signature: &str) -> Option<String> {
    serde_json::from_str::<Value>(signature)
        .ok()?
        .get("thoughtSignature")?
        .as_str()
        .map(str::to_string)
}

/// An id for a call the API sent without one, unique within the session.
fn generated_call_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    format!(
        "{GENERATED_CALL_ID_PREFIX}{started:x}-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn is_generated_call_id(id: &str) -> bool {
    id.starts_with(GENERATED_CALL_ID_PREFIX)
}

/// The `streamGenerateContent` request for `request`. `provider` names the
/// registry provider, whose own call ids are sent back to it.
fn request_body(
    request: &AgentMessageRequest<'_>,
    provider: &str,
    features: ModelFeatures,
    options: &RequestOptions,
) -> Value {
    let mut contents = Vec::new();
    let mut calls = HashMap::new();
    for message in request.messages {
        let own = message
            .origin
            .as_ref()
            .is_some_and(|origin| origin.provider == provider);
        if let Some(content) = content_for(message, own, &mut calls) {
            contents.push(content);
        }
    }
    let mut body = json!({
        "contents": contents,
        "generationConfig": {"maxOutputTokens": request.max_tokens},
    });
    if !request.system.is_empty() {
        body["systemInstruction"] = json!({"parts": [{"text": request.system}]});
    }
    if !request.tools.is_empty() {
        let declarations = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    // JSON Schema as is; `parameters` takes only an OpenAPI
                    // subset.
                    "parametersJsonSchema": tool.input_schema,
                })
            })
            .collect::<Vec<_>>();
        body["tools"] = json!([{ "functionDeclarations": declarations }]);
    }
    // The effort feature marks a thinking model: others reject
    // thinkingConfig.
    if features.effort {
        let mut thinking = json!({"includeThoughts": true});
        if let Some(effort) = options.effort {
            thinking["thinkingLevel"] = Value::String(thinking_level(effort).to_string());
        }
        body["generationConfig"]["thinkingConfig"] = thinking;
    }
    body
}

/// EFFORT as a Gemini thinking level. Gemini has no level above `xhigh`.
fn thinking_level(effort: &str) -> &str {
    match effort {
        "max" => "xhigh",
        other => other,
    }
}

/// A call sent to the model: its name, and the Gemini id when it has one.
struct SentCall {
    name: String,
    id: Option<String>,
}

/// One history message as a Gemini `Content`, or `None` when nothing in it
/// can be sent. `calls` remembers each call so its result can name it.
fn content_for(
    message: &Message,
    own: bool,
    calls: &mut HashMap<String, SentCall>,
) -> Option<Value> {
    let role = match message.role {
        Role::Assistant => "model",
        Role::User | Role::System => "user",
    };
    let parts = match &message.content {
        MessageContent::Text(text) if text.is_empty() => Vec::new(),
        MessageContent::Text(text) => vec![json!({"text": text})],
        MessageContent::Blocks(blocks) => match message.role {
            Role::Assistant => model_parts(blocks, own, calls),
            Role::User | Role::System => user_parts(blocks, calls),
        },
    };
    (!parts.is_empty()).then(|| json!({"role": role, "parts": parts}))
}

/// The parts of a model message, with each thought signature on the part
/// that followed its thinking block.
fn model_parts(
    blocks: &[ContentBlock],
    own: bool,
    calls: &mut HashMap<String, SentCall>,
) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    let mut pending = None;
    let mut signed = false;
    let mut push = |mut part: Value, pending: &mut Option<String>| {
        if let Some(signature) = pending.take() {
            part["thoughtSignature"] = Value::String(signature);
            signed = true;
        }
        parts.push(part);
    };
    for block in blocks {
        match block {
            ContentBlock::Thinking { signature, .. } => {
                if let Some(signature) = decode_signature(signature) {
                    pending = Some(signature);
                }
            }
            ContentBlock::Text { text } if !text.is_empty() => {
                push(json!({"text": text}), &mut pending);
            }
            ContentBlock::ToolUse { id, name, input } => {
                let sent_id = (own && !is_generated_call_id(id)).then(|| id.clone());
                let mut call = Map::new();
                call.insert("name".into(), Value::String(name.clone()));
                call.insert("args".into(), input.clone());
                if let Some(id) = &sent_id {
                    call.insert("id".into(), Value::String(id.clone()));
                }
                calls.insert(
                    id.clone(),
                    SentCall {
                        name: name.clone(),
                        id: sent_id,
                    },
                );
                push(json!({"functionCall": call}), &mut pending);
            }
            ContentBlock::Text { .. }
            | ContentBlock::RedactedThinking { .. }
            | ContentBlock::ToolResult { .. } => {}
        }
    }
    // A signature the API sent on an empty text part at the end.
    if pending.is_some() {
        push(json!({"text": ""}), &mut pending);
    }
    // Gemini 3 and later refuse a call without a signature in the current
    // turn; one this model did not write has none to send.
    if !signed
        && let Some(first_call) = parts
            .iter_mut()
            .find(|part| part.get("functionCall").is_some())
    {
        first_call["thoughtSignature"] = Value::String(SKIP_SIGNATURE.into());
    }
    parts
}

/// The parts of a user message: tool results first, as Gemini expects all
/// the responses to a step together, then text.
fn user_parts(blocks: &[ContentBlock], calls: &HashMap<String, SentCall>) -> Vec<Value> {
    let mut parts = Vec::new();
    for block in blocks {
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } = block
        {
            let call = calls.get(tool_use_id);
            let key = if *is_error == Some(true) {
                "error"
            } else {
                "result"
            };
            let mut response = Map::new();
            response.insert(
                "name".into(),
                Value::String(call.map_or_else(|| "unknown".to_string(), |c| c.name.clone())),
            );
            response.insert("response".into(), json!({ key: content }));
            if let Some(id) = call.and_then(|call| call.id.as_ref()) {
                response.insert("id".into(), Value::String(id.clone()));
            }
            parts.push(json!({"functionResponse": response}));
        }
    }
    for block in blocks {
        if let ContentBlock::Text { text } = block
            && !text.is_empty()
        {
            parts.push(json!({"text": text}));
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::Origin;
    use crate::models::ModelRegistry;
    use crate::providers::test_server::serve_once;
    use crate::tools::tools_for_model;

    const GEMINI: &str = r#"
[providers.gemini]
wire = "google"
base_url = "https://generativelanguage.googleapis.com/v1beta"
api_key_env = "GEMINI_API_KEY"

[[models]]
alias = "gemini"
provider = "gemini"
id = "gemini-4-carbon"
features = ["effort"]
"#;

    fn provider(registry_text: &str, effort: Option<&str>) -> GoogleProvider {
        let registry = ModelRegistry::from_toml(registry_text).unwrap();
        let model = registry
            .models
            .iter()
            .find(|model| model.provider != "anthropic")
            .unwrap();
        let effort = effort.map(str::to_string);
        GoogleProvider::from_config(
            registry.provider(&model.provider).unwrap(),
            model,
            &|key: &str| match key {
                "GEMINI_API_KEY" | "LOCAL_KEY" => Some("secret-key".into()),
                "EFFORT" => effort.clone(),
                _ => None,
            },
        )
        .unwrap()
    }

    fn local(base_url: &str, auth: &str) -> GoogleProvider {
        provider(
            &format!(
                "[providers.local]\nwire = \"google\"\nbase_url = \"{base_url}/v1beta\"\napi_key_env = \"LOCAL_KEY\"\n{auth}\n[[models]]\nalias = \"m\"\nprovider = \"local\"\nid = \"gemini-test\"\nfeatures = [\"effort\"]"
            ),
            None,
        )
    }

    fn origin() -> Origin {
        Origin {
            provider: "local".into(),
            model: "gemini-test".into(),
        }
    }

    fn sse(events: &[Value]) -> String {
        events
            .iter()
            .map(|event| format!("data: {event}\r\n\r\n"))
            .collect()
    }

    fn body_of(request: &str) -> Value {
        serde_json::from_str(&request[request.find("\r\n\r\n").unwrap() + 4..]).unwrap()
    }

    async fn send(
        provider: &GoogleProvider,
        messages: &[Message],
    ) -> Result<ProviderResponse, ProviderError> {
        let tools = tools_for_model();
        let request = AgentMessageRequest::new("gemini-test", 100, messages, &tools);
        provider
            .send_message(
                &crate::provider_http_client().unwrap(),
                &request,
                &mut |_| {},
            )
            .await
    }

    #[test]
    fn requests_stream_from_the_model_with_the_goog_api_key() {
        let messages = [Message::user_text("hi".into())];
        let tools = tools_for_model();
        let request = AgentMessageRequest::new("gemini-4-carbon", 1000, &messages, &tools);
        let built = provider(GEMINI, Some("max"))
            .build_request(&crate::test_support::http_client(), &request)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            built.url().as_str(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-4-carbon:streamGenerateContent?alt=sse"
        );
        let key = &built.headers()["x-goog-api-key"];
        assert_eq!(key, "secret-key");
        assert!(key.is_sensitive());
        assert!(built.headers().get("authorization").is_none());
        let body: Value =
            serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(
            body["contents"],
            json!([{"role": "user", "parts": [{"text": "hi"}]}])
        );
        assert_eq!(
            body["systemInstruction"],
            json!({"parts": [{"text": request.system}]})
        );
        assert_eq!(
            body["generationConfig"],
            json!({"maxOutputTokens": 1000,
                "thinkingConfig": {"includeThoughts": true, "thinkingLevel": "xhigh"}})
        );
        let declaration = &body["tools"][0]["functionDeclarations"][0];
        assert!(declaration["name"].is_string());
        assert!(declaration["parametersJsonSchema"].is_object());
        assert!(declaration.get("parameters").is_none());
    }

    #[test]
    fn a_bearer_provider_sends_the_key_as_a_bearer_token() {
        let request = AgentMessageRequest::new("m", 10, &[], &[]);
        let built = local("https://opencode.ai/inference/google", "auth = \"bearer\"")
            .build_request(&crate::test_support::http_client(), &request)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            built.url().as_str(),
            "https://opencode.ai/inference/google/v1beta/models/gemini-test:streamGenerateContent?alt=sse"
        );
        let auth = &built.headers()["authorization"];
        assert_eq!(auth, "Bearer secret-key");
        assert!(auth.is_sensitive());
        assert!(built.headers().get("x-goog-api-key").is_none());
    }

    #[test]
    fn models_without_the_effort_feature_get_no_thinking_config() {
        let request = AgentMessageRequest::new("m", 10, &[], &[]);
        let body = request_body(
            &request,
            "gemini",
            ModelFeatures::default(),
            &RequestOptions::default(),
        );
        assert_eq!(body["generationConfig"], json!({"maxOutputTokens": 10}));
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn calls_from_another_model_get_the_documented_skip_signature() {
        let messages = [
            Message::user_text("read it".into()),
            Message::assistant_from(
                vec![ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "read_file".into(),
                    input: json!({"path": "a.rs"}),
                }],
                Origin {
                    provider: "anthropic".into(),
                    model: "claude-opus-5-5".into(),
                },
            ),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: "missing".into(),
                is_error: Some(true),
            }]),
        ];
        let request = AgentMessageRequest::new("m", 10, &messages, &[]);
        let body = request_body(
            &request,
            "gemini",
            ModelFeatures::default(),
            &RequestOptions::default(),
        );
        assert_eq!(
            body["contents"],
            json!([
                {"role": "user", "parts": [{"text": "read it"}]},
                {"role": "model", "parts": [{"functionCall": {"name": "read_file",
                    "args": {"path": "a.rs"}},
                    "thoughtSignature": "skip_thought_signature_validator"}]},
                {"role": "user", "parts": [{"functionResponse": {"name": "read_file",
                    "response": {"error": "missing"}}}]},
            ])
        );
    }

    #[test]
    fn a_trailing_signature_goes_back_on_an_empty_part_and_other_wires_thinking_is_dropped() {
        let messages = [Message::assistant_from(
            vec![
                ContentBlock::Thinking {
                    thinking: "plan".into(),
                    signature: "anthropic-signature".into(),
                },
                ContentBlock::Text {
                    text: "Done.".into(),
                },
                ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: encode_signature("sig-C"),
                },
            ],
            origin(),
        )];
        let request = AgentMessageRequest::new("m", 10, &messages, &[]);
        let body = request_body(
            &request,
            "local",
            ModelFeatures::default(),
            &RequestOptions::default(),
        );
        assert_eq!(
            body["contents"],
            json!([{"role": "model", "parts": [
                {"text": "Done."},
                {"text": "", "thoughtSignature": "sig-C"},
            ]}])
        );
    }

    #[tokio::test]
    async fn a_tool_round_trip_sends_the_signature_back_on_the_call() {
        let first = sse(&[
            json!({"candidates": [{"content": {"role": "model", "parts": [
                {"text": "Check the file.", "thought": true}]}}]}),
            json!({"candidates": [{"content": {"role": "model", "parts": [
                {"text": "Reading."}]}}]}),
            json!({"candidates": [{"content": {"role": "model", "parts": [
                {"functionCall": {"id": "fc_1", "name": "read_file",
                    "args": {"path": "a.rs"}}, "thoughtSignature": "sig-A"}]},
                "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 50, "candidatesTokenCount": 5,
                    "thoughtsTokenCount": 7}}),
        ]);
        let (base_url, server) = serve_once("200 OK", first).await;
        let mut history = vec![Message::user_text("read a.rs".into())];
        let response = send(&local(&base_url, ""), &history).await.unwrap();
        let usage = response.usage.unwrap();
        assert_eq!((usage.input_tokens, usage.output_tokens), (50, 12));
        let request = server.await.unwrap();
        assert!(
            request.starts_with("POST /v1beta/models/gemini-test:streamGenerateContent?alt=sse "),
            "{request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-goog-api-key: secret-key")
        );

        history.push(Message::assistant_from(response.message.content, origin()));
        history.push(Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "fc_1".into(),
            content: "fn main() {}".into(),
            is_error: None,
        }]));
        let second = sse(&[json!({"candidates": [{"content": {"role": "model",
            "parts": [{"text": "It is empty."}]}, "finishReason": "STOP"}]})]);
        let (base_url, server) = serve_once("200 OK", second).await;
        let response = send(&local(&base_url, ""), &history).await.unwrap();
        assert_eq!(
            response.message.content,
            [ContentBlock::Text {
                text: "It is empty.".into()
            }]
        );
        let contents = body_of(&server.await.unwrap())["contents"].clone();
        assert_eq!(
            contents,
            json!([
                {"role": "user", "parts": [{"text": "read a.rs"}]},
                {"role": "model", "parts": [
                    {"text": "Reading."},
                    {"functionCall": {"id": "fc_1", "name": "read_file",
                        "args": {"path": "a.rs"}}, "thoughtSignature": "sig-A"},
                ]},
                {"role": "user", "parts": [{"functionResponse": {"id": "fc_1",
                    "name": "read_file", "response": {"result": "fn main() {}"}}}]},
            ])
        );
    }

    #[tokio::test]
    async fn rate_limits_keep_the_status_and_are_retried() {
        let (base_url, _server) = serve_once(
            "429 Too Many Requests",
            r#"{"error":{"code":429,"message":"Resource has been exhausted (e.g. check quota).","status":"RESOURCE_EXHAUSTED"}}"#
                .into(),
        )
        .await;
        let err = send(&local(&base_url, ""), &[Message::user_text("hi".into())])
            .await
            .err()
            .unwrap();
        assert!(err.is_retryable());
        assert_eq!(
            err.to_string(),
            "local API error 429 Too Many Requests: RESOURCE_EXHAUSTED: Resource has been exhausted (e.g. check quota)."
        );
    }
}
