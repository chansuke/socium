//! OpenAI Chat Completions (`wire = "openai-chat"`): OpenAI itself, and
//! Ollama, OpenRouter, and other OpenAI-compatible endpoints. Conversation
//! history is converted here, so the rest of core keeps one message format.

mod stream;

use crate::agent::types::{AgentMessageRequest, ContentBlock, Message, MessageContent, Role};
use crate::models::{ModelConfig, ModelFeatures, ProviderConfig};
use crate::providers::anthropic::request_options::RequestOptions;
use crate::providers::error::{ProviderError, api_error};
use crate::providers::sse::SseParser;
use crate::providers::{
    Provider, ProviderCapabilities, ProviderEvent, ProviderKind, ProviderResponse, bearer,
};
use crate::tools::ToolDefinition;
use serde_json::{Value, json};
use std::io::{Error, ErrorKind};
use stream::ChatStreamBuilder;

pub(crate) struct OpenAiChatProvider {
    name: String,
    completions_url: String,
    /// `None` for a local endpoint that takes no key.
    api_key: Option<String>,
    features: ModelFeatures,
    options: RequestOptions,
}

impl OpenAiChatProvider {
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
            completions_url: format!("{}/chat/completions", provider.base_url),
            api_key,
            features: model.features,
            options,
        })
    }

    fn build_request(
        &self,
        client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        let body = request_body(request, self.features, &self.options)?;
        let mut http_request = client.post(&self.completions_url);
        if let Some(api_key) = &self.api_key {
            http_request = http_request.header(reqwest::header::AUTHORIZATION, bearer(api_key)?);
        }
        Ok(http_request.json(&body))
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiChatProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAiChat
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

    /// Chat Completions has no field for thinking; nothing is sent back.
    fn reads_thinking_from(&self, _: Option<&crate::agent::types::Origin>) -> bool {
        false
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
        let mut builder = ChatStreamBuilder::default();
        let mut forward = |event: ProviderEvent| on_delta(event);
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(crate::providers::error::transport)?
        {
            for data in parser.push(&chunk)? {
                forward(ProviderEvent::Raw { json: data.clone() });
                builder.apply_chunk(&data, &mut forward)?;
            }
        }
        if let Some(data) = parser.finish()? {
            forward(ProviderEvent::Raw { json: data.clone() });
            builder.apply_chunk(&data, &mut forward)?;
        }
        builder.finish(&mut forward)
    }
}

/// The Chat Completions request for `request`.
fn request_body(
    request: &AgentMessageRequest<'_>,
    features: ModelFeatures,
    options: &RequestOptions,
) -> Result<Value, ProviderError> {
    let mut messages = vec![json!({"role": "system", "content": request.system})];
    for message in request.messages {
        append_message(&mut messages, message);
    }
    let tools = request
        .tools
        .iter()
        .map(|tool| match tool {
            ToolDefinition::Custom(custom) => Ok(json!({
                "type": "function",
                "function": {
                    "name": custom.name,
                    "description": custom.description,
                    "parameters": custom.input_schema,
                },
            })),
            ToolDefinition::AnthropicTextEditor(editor) => Err(ProviderError::Api(format!(
                "{} is an Anthropic built-in tool and cannot be sent to an OpenAI endpoint",
                editor.name
            ))),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
        "max_completion_tokens": request.max_tokens,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(effort) = options.effort.filter(|_| features.effort) {
        body["reasoning_effort"] = Value::String(effort.to_string());
    }
    Ok(body)
}

/// Appends one history message in Chat Completions form. Tool results become
/// `tool` messages, which must directly follow the assistant's tool calls.
/// Thinking is dropped: its signature is meaningless to another provider.
fn append_message(messages: &mut Vec<Value>, message: &Message) {
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    };
    let blocks = match &message.content {
        MessageContent::Text(text) => {
            messages.push(json!({"role": role, "content": text}));
            return;
        }
        MessageContent::Blocks(blocks) => blocks,
    };
    let text = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    match message.role {
        Role::Assistant => {
            let tool_calls = blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, input } => Some(json!({
                        "id": id,
                        "type": "function",
                        "function": {"name": name, "arguments": input.to_string()},
                    })),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let mut assistant = json!({"role": "assistant", "content": text});
            if !tool_calls.is_empty() {
                if text.is_empty() {
                    assistant["content"] = Value::Null;
                }
                assistant["tool_calls"] = Value::Array(tool_calls);
            }
            messages.push(assistant);
        }
        Role::User | Role::System => {
            for block in blocks {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = block
                {
                    let content = if *is_error == Some(true) {
                        format!("Error: {content}")
                    } else {
                        content.clone()
                    };
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": content,
                    }));
                }
            }
            if !text.is_empty() {
                messages.push(json!({"role": role, "content": text}));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ModelRegistry;
    use crate::providers::test_server::serve_once;
    use crate::tools::tools_for_model;

    const OPENAI: &str = r#"
[providers.openai]
wire = "openai-chat"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"

[[models]]
alias = "gpt"
provider = "openai"
id = "gpt-6-astra"
features = ["effort"]
"#;

    fn provider(effort: Option<&str>) -> OpenAiChatProvider {
        let registry = ModelRegistry::from_toml(OPENAI).unwrap();
        let model = registry.find("gpt").unwrap();
        let effort = effort.map(str::to_string);
        OpenAiChatProvider::from_config(
            registry.provider("openai").unwrap(),
            model,
            &|key: &str| match key {
                "OPENAI_API_KEY" => Some("secret-key".into()),
                "EFFORT" => effort.clone(),
                _ => None,
            },
        )
        .unwrap()
    }

    fn history() -> Vec<Message> {
        vec![
            Message::user_text("read it".into()),
            Message::assistant_blocks(vec![
                ContentBlock::Thinking {
                    thinking: "private reasoning".into(),
                    signature: "sig".into(),
                },
                ContentBlock::Text {
                    text: "Reading.".into(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "read_file".into(),
                    input: json!({"path": "README.md"}),
                },
            ]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: "missing".into(),
                is_error: Some(true),
            }]),
        ]
    }

    #[test]
    fn requests_go_to_chat_completions_with_a_bearer_key() {
        let messages = history();
        let tools = tools_for_model(false);
        let request = AgentMessageRequest::new("gpt-6-astra", 1000, &messages, &tools);
        let built = provider(Some("high"))
            .build_request(&reqwest::Client::new(), &request)
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            built.url().as_str(),
            "https://api.openai.com/v1/chat/completions"
        );
        let auth = &built.headers()["authorization"];
        assert_eq!(auth, "Bearer secret-key");
        assert!(auth.is_sensitive());
        let body: Value =
            serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["model"], "gpt-6-astra");
        assert_eq!(body["max_completion_tokens"], 1000);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["reasoning_effort"], "high");
        assert!(body.get("thinking").is_none());
        let tool = &body["tools"][0];
        assert_eq!(tool["type"], "function");
        assert!(tool["function"]["parameters"].is_object());
    }

    #[test]
    fn history_becomes_chat_messages_without_thinking() {
        let messages = history();
        let request = AgentMessageRequest::new("m", 10, &messages, &[]);
        let body = request_body(
            &request,
            ModelFeatures::default(),
            &RequestOptions::default(),
        )
        .unwrap();
        let sent = body["messages"].as_array().unwrap();
        assert_eq!(sent.len(), 4);
        assert_eq!(sent[0]["role"], "system");
        assert_eq!(sent[1], json!({"role": "user", "content": "read it"}));
        assert_eq!(sent[2]["role"], "assistant");
        assert_eq!(sent[2]["content"], "Reading.");
        assert_eq!(sent[2]["tool_calls"][0]["id"], "toolu_1");
        assert_eq!(
            sent[2]["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"README.md"}"#
        );
        assert_eq!(
            sent[3],
            json!({"role": "tool", "tool_call_id": "toolu_1", "content": "Error: missing"})
        );
        assert!(!body.to_string().contains("private reasoning"));
        assert!(body.get("tools").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn a_tool_call_without_text_has_null_content() {
        let messages = vec![Message::assistant_blocks(vec![ContentBlock::ToolUse {
            id: "call_1".into(),
            name: "git_status".into(),
            input: json!({}),
        }])];
        let request = AgentMessageRequest::new("m", 10, &messages, &[]);
        let body = request_body(
            &request,
            ModelFeatures::default(),
            &RequestOptions::default(),
        )
        .unwrap();
        assert!(body["messages"][1]["content"].is_null());
    }

    #[test]
    fn the_anthropic_text_editor_type_is_refused() {
        let tools = tools_for_model(true);
        let request = AgentMessageRequest::new("m", 10, &[], &tools);
        let err = request_body(
            &request,
            ModelFeatures::default(),
            &RequestOptions::default(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Anthropic built-in tool"));
    }

    fn local_provider(base_url: &str) -> OpenAiChatProvider {
        let registry = ModelRegistry::from_toml(&format!(
            "[providers.local]\nwire = \"openai-chat\"\nbase_url = \"{base_url}/v1\"\napi_key_env = \"LOCAL_KEY\"\n[[models]]\nalias = \"m\"\nprovider = \"local\"\nid = \"local-model\""
        ))
        .unwrap();
        OpenAiChatProvider::from_config(
            registry.provider("local").unwrap(),
            registry.find("m").unwrap(),
            &|key: &str| (key == "LOCAL_KEY").then(|| "local-key".to_string()),
        )
        .unwrap()
    }

    fn sse(chunks: &[Value]) -> String {
        chunks
            .iter()
            .map(|chunk| format!("data: {chunk}\n\n"))
            .chain(std::iter::once("data: [DONE]\n\n".to_string()))
            .collect()
    }

    async fn send(provider: &OpenAiChatProvider) -> Result<ProviderResponse, ProviderError> {
        let messages = [Message::user_text("read a.rs".into())];
        let tools = tools_for_model(false);
        let request = AgentMessageRequest::new("local-model", 100, &messages, &tools);
        provider
            .send_message(
                &crate::provider_http_client().unwrap(),
                &request,
                &mut |_| {},
            )
            .await
    }

    #[tokio::test]
    async fn a_local_endpoint_streams_a_tool_call() {
        let body = sse(&[
            json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_1",
                "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":"}}]}}]}),
            json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0,
                "function": {"arguments": "\"a.rs\"}"}}]}, "finish_reason": "tool_calls"}]}),
            json!({"choices": [], "usage": {"prompt_tokens": 50, "completion_tokens": 5}}),
        ]);
        let (base_url, server) = serve_once("200 OK", body).await;
        let response = send(&local_provider(&base_url)).await.unwrap();
        assert_eq!(
            response.message.content,
            [ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "read_file".into(),
                input: json!({"path": "a.rs"}),
            }]
        );
        assert_eq!(response.usage.unwrap().input_tokens, 50);

        let request = server.await.unwrap();
        assert!(request.starts_with("POST /v1/chat/completions "));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer local-key")
        );
        assert!(request.contains("\"stream\":true"));
    }

    #[tokio::test]
    async fn a_local_endpoint_without_a_key_gets_no_authorization_header() {
        let registry = ModelRegistry::from_toml(&format!(
            "[providers.local]\nwire = \"openai-chat\"\nbase_url = \"{}/v1\"\n[[models]]\nalias = \"m\"\nprovider = \"local\"\nid = \"local-model\"",
            "http://127.0.0.1:1"
        ))
        .unwrap();
        let body = sse(&[
            json!({"choices": [{"index": 0, "delta": {"content": "hi"}, "finish_reason": "stop"}]}),
        ]);
        let (base_url, server) = serve_once("200 OK", body).await;
        let mut provider = OpenAiChatProvider::from_config(
            registry.provider("local").unwrap(),
            registry.find("m").unwrap(),
            &|_| None,
        )
        .unwrap();
        provider.completions_url = format!("{base_url}/v1/chat/completions");
        send(&provider).await.unwrap();
        let request = server.await.unwrap().to_ascii_lowercase();
        assert!(!request.contains("authorization:"), "{request}");
    }

    #[tokio::test]
    async fn http_errors_show_the_provider_and_code() {
        let (base_url, _server) = serve_once(
            "401 Unauthorized",
            r#"{"error":{"message":"Incorrect API key","type":"invalid_request_error","code":"invalid_api_key"}}"#
                .into(),
        )
        .await;
        let err = send(&local_provider(&base_url)).await.err().unwrap();
        assert_eq!(
            err.to_string(),
            "local API error 401 Unauthorized: invalid_request_error: Incorrect API key"
        );
    }

    #[tokio::test]
    async fn a_stream_cut_before_its_finish_reason_fails() {
        let body = format!(
            "data: {}\n\n",
            json!({"choices": [{"index": 0, "delta": {"content": "partial"}}]})
        );
        let (base_url, _server) = serve_once("200 OK", body).await;
        let err = send(&local_provider(&base_url)).await.err().unwrap();
        assert!(err.to_string().contains("without a finish reason"));
    }
}
