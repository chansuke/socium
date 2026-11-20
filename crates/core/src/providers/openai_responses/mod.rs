//! OpenAI Responses (`wire = "openai-responses"`). Unlike Chat Completions,
//! reasoning models may call tools while reasoning, and the encrypted
//! reasoning items are sent back on the next request so the model keeps its
//! reasoning across tool calls. No state is stored on the server
//! (`store: false`); the full history is sent each time.

mod stream;

use crate::conversation::{AgentMessageRequest, ContentBlock, Message, MessageContent, Role};
use crate::models::{ModelConfig, ModelFeatures, ProviderConfig};
use crate::providers::anthropic::request_options::RequestOptions;
use crate::providers::error::{ProviderError, api_error};
use crate::providers::sse::SseParser;
use crate::providers::{
    Provider, ProviderCapabilities, ProviderEvent, ProviderKind, ProviderResponse, bearer,
};
use serde_json::{Value, json};
use std::io::{Error, ErrorKind};
use stream::ResponsesStreamBuilder;

pub(crate) struct OpenAiResponsesProvider {
    name: String,
    model: String,
    responses_url: String,
    /// `None` for a local endpoint that takes no key.
    api_key: Option<String>,
    features: ModelFeatures,
    options: RequestOptions,
    /// `tools = false` in models.toml.
    supports_tools: bool,
}

impl OpenAiResponsesProvider {
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
            responses_url: format!("{}/responses", provider.base_url),
            api_key,
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
        let body = request_body(request, self.features, &self.options)?;
        let mut http_request = client.post(&self.responses_url);
        if let Some(api_key) = &self.api_key {
            http_request = http_request.header(reqwest::header::AUTHORIZATION, bearer(api_key)?);
        }
        Ok(http_request.json(&body))
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiResponsesProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAiResponses
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

    /// Encrypted reasoning items go back only to the model that wrote them.
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
        let mut builder = ResponsesStreamBuilder::default();
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

/// The Responses request for `request`.
fn request_body(
    request: &AgentMessageRequest<'_>,
    features: ModelFeatures,
    options: &RequestOptions,
) -> Result<Value, ProviderError> {
    let mut input = Vec::new();
    for message in request.messages {
        append_items(&mut input, message);
    }
    let tools = request
        .tools
        .iter()
        .map(|custom| {
            json!({
                "type": "function",
                "name": custom.name,
                "description": custom.description,
                "parameters": custom.input_schema,
                // Strict mode needs every property listed as required,
                // which these schemas do not do; say so rather than rely on
                // the endpoint's default.
                "strict": false,
            })
        })
        .collect::<Vec<_>>();

    let mut body = json!({
        "model": request.model,
        "instructions": request.system,
        "input": input,
        "stream": true,
        "store": false,
        "max_output_tokens": request.max_tokens,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    // The effort feature marks a reasoning model: other models reject the
    // reasoning fields.
    if features.effort {
        let mut reasoning = json!({"summary": "auto"});
        if let Some(effort) = options.effort {
            reasoning["effort"] = Value::String(effort.to_string());
        }
        body["reasoning"] = reasoning;
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    Ok(body)
}

/// Appends one history message as Responses input items. Thinking is sent
/// only when it holds a reasoning item from this wire; an Anthropic
/// signature is meaningless here.
fn append_items(items: &mut Vec<Value>, message: &Message) {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    };
    let blocks = match &message.content {
        MessageContent::Text(text) => {
            items.push(match message.role {
                Role::Assistant => assistant_message(text, "final_answer"),
                Role::User | Role::System => json!({"role": role, "content": text}),
            });
            return;
        }
        MessageContent::Blocks(blocks) => blocks,
    };
    match message.role {
        Role::Assistant => {
            // Text before a tool call is commentary; the rest is the answer.
            let phase = if blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
            {
                "commentary"
            } else {
                "final_answer"
            };
            for block in blocks {
                match block {
                    ContentBlock::Thinking { signature, .. } => {
                        if let Some(item) = replayable_reasoning(signature) {
                            items.push(item);
                        }
                    }
                    ContentBlock::Text { text } if !text.is_empty() => {
                        items.push(assistant_message(text, phase));
                    }
                    ContentBlock::ToolUse { id, name, input } => items.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": input.to_string(),
                    })),
                    ContentBlock::Text { .. }
                    | ContentBlock::RedactedThinking { .. }
                    | ContentBlock::ToolResult { .. } => {}
                }
            }
        }
        Role::User | Role::System => {
            let mut text = Vec::new();
            for block in blocks {
                match block {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let output = if *is_error == Some(true) {
                            format!("Error: {content}")
                        } else {
                            content.clone()
                        };
                        items.push(json!({
                            "type": "function_call_output",
                            "call_id": tool_use_id,
                            "output": output,
                        }));
                    }
                    ContentBlock::Text { text: part } => text.push(part.as_str()),
                    ContentBlock::Thinking { .. }
                    | ContentBlock::RedactedThinking { .. }
                    | ContentBlock::ToolUse { .. } => {}
                }
            }
            if !text.is_empty() {
                items.push(json!({"role": role, "content": text.join("\n")}));
            }
        }
    }
}

fn assistant_message(text: &str, phase: &str) -> Value {
    json!({"role": "assistant", "content": text, "phase": phase})
}

/// The reasoning item stored in a thinking signature by this wire's stream.
fn replayable_reasoning(signature: &str) -> Option<Value> {
    serde_json::from_str::<Value>(signature)
        .ok()
        .filter(|item| item["type"] == "reasoning")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ModelRegistry;
    use crate::providers::test_server::serve_once;
    use crate::tools::tools_for_model;

    const OPENAI: &str = r#"
[providers.openai]
wire = "openai-responses"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[[models]]
alias = "gpt"
provider = "openai"
id = "gpt-6-astra"
features = ["effort"]
"#;

    fn provider(effort: Option<&str>) -> OpenAiResponsesProvider {
        let registry = ModelRegistry::from_toml(OPENAI).unwrap();
        let effort = effort.map(str::to_string);
        OpenAiResponsesProvider::from_config(
            registry.provider("openai").unwrap(),
            registry.find("gpt").unwrap(),
            &|key: &str| match key {
                "OPENAI_API_KEY" => Some("secret-key".into()),
                "EFFORT" => effort.clone(),
                _ => None,
            },
        )
        .unwrap()
    }

    fn reasoning_features() -> ModelFeatures {
        ModelFeatures {
            effort: true,
            ..ModelFeatures::default()
        }
    }

    fn replayed_reasoning() -> String {
        json!({"type": "reasoning", "summary": [], "encrypted_content": "opaque"}).to_string()
    }

    fn items(messages: &[Message]) -> Vec<Value> {
        let mut items = Vec::new();
        for message in messages {
            append_items(&mut items, message);
        }
        items
    }

    #[test]
    fn requests_go_to_responses_without_server_state() {
        let messages = [Message::user_text("hi".into())];
        let tools = tools_for_model();
        let request = AgentMessageRequest::new("gpt-6-astra", 1000, &messages, &tools);
        let built = provider(Some("high"))
            .build_request(&crate::test_support::http_client(), &request)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(built.url().as_str(), "https://api.openai.com/v1/responses");
        let auth = &built.headers()["authorization"];
        assert_eq!(auth, "Bearer secret-key");
        assert!(auth.is_sensitive());
        let body: Value =
            serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["model"], "gpt-6-astra");
        assert_eq!(body["instructions"], request.system);
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["max_output_tokens"], 1000);
        assert_eq!(
            body["reasoning"],
            json!({"effort": "high", "summary": "auto"})
        );
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(body["input"], json!([{"role": "user", "content": "hi"}]));
        let tool = &body["tools"][0];
        assert_eq!(tool["type"], "function");
        assert!(tool["name"].is_string());
        assert!(tool["parameters"].is_object());
        // gpt-6-astra rejects these schemas with strict: true (checked
        // 2026-11-14).
        assert_eq!(tool["strict"], false);
    }

    #[test]
    fn models_without_the_effort_feature_get_no_reasoning_fields() {
        let request = AgentMessageRequest::new("m", 10, &[], &[]);
        let body = request_body(
            &request,
            ModelFeatures::default(),
            &RequestOptions::default(),
        )
        .unwrap();
        assert!(body.get("reasoning").is_none());
        assert!(body.get("include").is_none());
        assert!(body.get("tools").is_none());

        let body =
            request_body(&request, reasoning_features(), &RequestOptions::default()).unwrap();
        assert_eq!(body["reasoning"], json!({"summary": "auto"}));
    }

    #[test]
    fn history_becomes_input_items_with_reasoning_replayed() {
        let messages = [
            Message::user_text("read it".into()),
            Message::assistant_blocks(vec![
                ContentBlock::Thinking {
                    thinking: "plan".into(),
                    signature: replayed_reasoning(),
                },
                ContentBlock::Text {
                    text: "Reading.".into(),
                },
                ContentBlock::ToolUse {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    input: json!({"path": "README.md"}),
                },
            ]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "missing".into(),
                is_error: Some(true),
            }]),
            Message::assistant_blocks(vec![ContentBlock::Text {
                text: "It is missing.".into(),
            }]),
        ];
        assert_eq!(
            items(&messages),
            [
                json!({"role": "user", "content": "read it"}),
                serde_json::from_str::<Value>(&replayed_reasoning()).unwrap(),
                json!({"role": "assistant", "content": "Reading.", "phase": "commentary"}),
                json!({"type": "function_call", "call_id": "call_1", "name": "read_file",
                    "arguments": r#"{"path":"README.md"}"#}),
                json!({"type": "function_call_output", "call_id": "call_1",
                    "output": "Error: missing"}),
                json!({"role": "assistant", "content": "It is missing.",
                    "phase": "final_answer"}),
            ]
        );
    }

    #[test]
    fn thinking_from_another_wire_is_not_sent() {
        let messages = [Message::assistant_blocks(vec![
            ContentBlock::Thinking {
                thinking: "plan".into(),
                signature: "anthropic-signature".into(),
            },
            ContentBlock::Text { text: "Hi.".into() },
        ])];
        assert_eq!(
            items(&messages),
            [json!({"role": "assistant", "content": "Hi.", "phase": "final_answer"})]
        );
    }

    #[test]
    fn the_text_editor_goes_as_an_ordinary_function() {
        let tools = tools_for_model();
        let request = AgentMessageRequest::new("m", 10, &[], &tools);
        let body = request_body(
            &request,
            ModelFeatures::default(),
            &RequestOptions::default(),
        )
        .unwrap();
        let editor = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == crate::constants::TEXT_EDITOR_TOOL_NAME)
            .unwrap();
        assert!(
            editor["parameters"]["properties"]["command"].is_object(),
            "{editor}"
        );
        assert!(editor.get("max_characters").is_none());
    }

    fn local_provider(base_url: &str) -> OpenAiResponsesProvider {
        let registry = ModelRegistry::from_toml(&format!(
            "[providers.local]\nwire = \"openai-responses\"\nbase_url = \"{base_url}/v1\"\napi_key_env = \"LOCAL_KEY\"\n[[models]]\nalias = \"m\"\nprovider = \"local\"\nid = \"local-model\"\nfeatures = [\"effort\"]"
        ))
        .unwrap();
        OpenAiResponsesProvider::from_config(
            registry.provider("local").unwrap(),
            registry.find("m").unwrap(),
            &|key: &str| (key == "LOCAL_KEY").then(|| "local-key".to_string()),
        )
        .unwrap()
    }

    /// Responses streams name each event twice: in `event:` and in `type`.
    fn sse(events: &[Value]) -> String {
        events
            .iter()
            .map(|event| {
                format!(
                    "event: {}\ndata: {event}\n\n",
                    event["type"].as_str().unwrap()
                )
            })
            .collect()
    }

    async fn send(
        provider: &OpenAiResponsesProvider,
        messages: &[Message],
    ) -> Result<ProviderResponse, ProviderError> {
        let tools = tools_for_model();
        let request = AgentMessageRequest::new("local-model", 100, messages, &tools);
        provider
            .send_message(
                &crate::provider_http_client().unwrap(),
                &request,
                &mut |_| {},
            )
            .await
    }

    fn body_of(request: &str) -> Value {
        serde_json::from_str(&request[request.find("\r\n\r\n").unwrap() + 4..]).unwrap()
    }

    #[tokio::test]
    async fn a_tool_round_trip_sends_the_reasoning_back() {
        let first = sse(&[
            json!({"type": "response.output_item.done", "output_index": 0,
                "item": {"type": "reasoning", "id": "rs_1", "summary": [],
                    "encrypted_content": "opaque"}}),
            json!({"type": "response.output_item.done", "output_index": 1,
                "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1",
                    "name": "read_file", "arguments": "{\"path\":\"a.rs\"}"}}),
            json!({"type": "response.completed", "response": {"status": "completed",
                "usage": {"input_tokens": 50, "output_tokens": 5}}}),
        ]);
        let (base_url, server) = serve_once("200 OK", first).await;
        let mut history = vec![Message::user_text("read a.rs".into())];
        let response = send(&local_provider(&base_url), &history).await.unwrap();
        assert_eq!(response.usage.unwrap().input_tokens, 50);
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /v1/responses "));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer local-key")
        );

        history.push(Message::assistant_blocks(response.message.content));
        history.push(Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "call_1".into(),
            content: "fn main() {}".into(),
            is_error: None,
        }]));
        let second = sse(&[
            json!({"type": "response.output_item.done", "output_index": 0,
                "item": {"type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": "It is empty."}]}}),
            json!({"type": "response.completed", "response": {"status": "completed"}}),
        ]);
        let (base_url, server) = serve_once("200 OK", second).await;
        let response = send(&local_provider(&base_url), &history).await.unwrap();
        assert_eq!(
            response.message.content,
            [ContentBlock::Text {
                text: "It is empty.".into()
            }]
        );
        let input = body_of(&server.await.unwrap())["input"].clone();
        assert_eq!(
            input,
            json!([
                {"role": "user", "content": "read a.rs"},
                {"type": "reasoning", "summary": [], "encrypted_content": "opaque"},
                {"type": "function_call", "call_id": "call_1", "name": "read_file",
                    "arguments": "{\"path\":\"a.rs\"}"},
                {"type": "function_call_output", "call_id": "call_1", "output": "fn main() {}"},
            ])
        );
    }

    #[tokio::test]
    async fn http_errors_show_the_provider_and_code() {
        let (base_url, _server) = serve_once(
            "400 Bad Request",
            r#"{"error":{"message":"Unsupported parameter","type":"invalid_request_error","code":null}}"#
                .into(),
        )
        .await;
        let err = send(
            &local_provider(&base_url),
            &[Message::user_text("hi".into())],
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            err.to_string(),
            "local API error 400 Bad Request: invalid_request_error: Unsupported parameter"
        );
    }
}
