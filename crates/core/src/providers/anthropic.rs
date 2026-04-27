use crate::agent::types::{AgentMessageRequest, AgentMessageResponse, ContentBlock};
use crate::constants::ANTHROPIC_MESSAGES_API;
use serde::Deserialize;
use serde_json::Value;
use std::io::Error;

use super::{DeltaEvent, ProviderCapabilities, ProviderResponse};

pub const CAPABILITIES: ProviderCapabilities = ProviderCapabilities {
    supports_tools: true,
};

pub(crate) async fn send_message<F>(
    client: &reqwest::Client,
    api_key: &str,
    anthropic_version: &str,
    message_request: &AgentMessageRequest<'_>,
    on_delta: &mut F,
) -> Result<ProviderResponse, Box<dyn std::error::Error>>
where
    F: FnMut(DeltaEvent) + Send,
{
    let mut body = serde_json::to_value(message_request)?;
    body["stream"] = Value::Bool(true);
    if let Ok(budget_str) = std::env::var("THINKING_BUDGET_TOKENS")
        && let Ok(budget) = budget_str.parse::<u64>()
    {
        body["thinking"] = serde_json::json!({
            "type": "enabled",
            "budget_tokens": budget
        });
    }
    let mut response = client
        .post(ANTHROPIC_MESSAGES_API)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", anthropic_version)
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await?;
        return Err(Error::other(format!("Anthropic API error {status}: {error_body}")).into());
    }

    let mut parser = SseParser::default();
    let mut builder = StreamingMessageBuilder::default();

    while let Some(chunk) = response.chunk().await? {
        for event in parser.push(&chunk)? {
            builder.apply_event(&event, on_delta)?;
        }
    }

    if let Some(event) = parser.finish()? {
        builder.apply_event(&event, on_delta)?;
    }

    let streamed_text = builder.streamed_text;
    let message = builder.finish()?;
    Ok(ProviderResponse {
        message,
        streamed_text,
    })
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart,
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: StreamContentBlockStart,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: StreamDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop,
    #[serde(rename = "message_delta")]
    MessageDelta,
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: AnthropicErrorPayload },
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorPayload {
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum StreamContentBlockStart {
    #[serde(rename = "text")]
    Text {
        #[serde(default)]
        text: String,
    },
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum StreamDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "signature_delta")]
    Signature { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
}

#[derive(Default)]
struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some((event_end, separator_len)) = find_event_boundary(&self.buffer) {
            let event_bytes = self.buffer[..event_end].to_vec();
            self.buffer.drain(..event_end + separator_len);
            if let Some(event) = parse_sse_event(&event_bytes)? {
                events.push(event);
            }
        }

        Ok(events)
    }

    fn finish(&mut self) -> Result<Option<String>, Box<dyn std::error::Error>> {
        if self.buffer.is_empty() {
            return Ok(None);
        }
        let remaining = std::mem::take(&mut self.buffer);
        parse_sse_event(&remaining)
    }
}

fn find_event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
        if bytes[index..].starts_with(b"\n\n") {
            return Some((index, 2));
        }
        index += 1;
    }
    None
}

fn parse_sse_event(bytes: &[u8]) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let text = std::str::from_utf8(bytes)?;
    let mut data_lines = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            let data = rest.strip_prefix(' ').unwrap_or(rest);
            data_lines.push(data);
        }
    }

    if data_lines.is_empty() {
        return Ok(None);
    }

    Ok(Some(data_lines.join("\n")))
}

#[derive(Default)]
struct StreamingMessageBuilder {
    blocks: Vec<Option<StreamingContentBlock>>,
    text_block_count: usize,
    streamed_text: bool,
}

enum StreamingContentBlock {
    Text(TextBlockState),
    Thinking(ThinkingBlockState),
    ToolUse(ToolUseBlockState),
}

struct TextBlockState {
    text: String,
    separator_pending: bool,
}

struct ThinkingBlockState {
    thinking: String,
    signature: String,
}

struct ToolUseBlockState {
    id: String,
    name: String,
    initial_input: Value,
    input_json: String,
}

impl StreamingMessageBuilder {
    fn apply_event(
        &mut self,
        raw_event: &str,
        on_delta: &mut impl FnMut(DeltaEvent),
    ) -> Result<(), Box<dyn std::error::Error>> {
        let event: AnthropicStreamEvent = serde_json::from_str(raw_event)?;
        match event {
            AnthropicStreamEvent::MessageStart
            | AnthropicStreamEvent::ContentBlockStop
            | AnthropicStreamEvent::MessageDelta
            | AnthropicStreamEvent::MessageStop
            | AnthropicStreamEvent::Ping => {}
            AnthropicStreamEvent::Error { error } => {
                return Err(
                    Error::other(format!("Anthropic streaming error: {}", error.message)).into(),
                );
            }
            AnthropicStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => self.start_block(index, content_block, on_delta)?,
            AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                self.apply_delta(index, delta, on_delta)?
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<AgentMessageResponse, Box<dyn std::error::Error>> {
        let mut content = Vec::with_capacity(self.blocks.len());
        for block in self.blocks {
            let Some(block) = block else {
                continue;
            };
            content.push(match block {
                StreamingContentBlock::Text(state) => ContentBlock::Text { text: state.text },
                StreamingContentBlock::Thinking(state) => ContentBlock::Thinking {
                    thinking: state.thinking,
                    signature: state.signature,
                },
                StreamingContentBlock::ToolUse(state) => {
                    let input = if state.input_json.trim().is_empty() {
                        state.initial_input
                    } else {
                        serde_json::from_str(&state.input_json)?
                    };
                    ContentBlock::ToolUse {
                        id: state.id,
                        name: state.name,
                        input,
                    }
                }
            });
        }
        Ok(AgentMessageResponse { content })
    }

    fn start_block(
        &mut self,
        index: usize,
        content_block: StreamContentBlockStart,
        on_delta: &mut impl FnMut(DeltaEvent),
    ) -> Result<(), Box<dyn std::error::Error>> {
        ensure_block_capacity(&mut self.blocks, index);
        let block = match content_block {
            StreamContentBlockStart::Text { text } => {
                let separator_pending = self.text_block_count > 0;
                self.text_block_count += 1;
                let mut state = TextBlockState {
                    text: String::new(),
                    separator_pending,
                };
                if !text.is_empty() {
                    let emitted = append_text_to_state(&mut state, &text);
                    if !emitted.is_empty() {
                        self.streamed_text = true;
                        on_delta(DeltaEvent::Text(emitted));
                    }
                }
                StreamingContentBlock::Text(state)
            }
            StreamContentBlockStart::Thinking { thinking } => {
                if !thinking.is_empty() {
                    on_delta(DeltaEvent::Thinking(thinking.clone()));
                }
                StreamingContentBlock::Thinking(ThinkingBlockState {
                    thinking,
                    signature: String::new(),
                })
            }
            StreamContentBlockStart::ToolUse { id, name, input } => {
                StreamingContentBlock::ToolUse(ToolUseBlockState {
                    id,
                    name,
                    initial_input: input,
                    input_json: String::new(),
                })
            }
        };
        self.blocks[index] = Some(block);
        Ok(())
    }

    fn apply_delta(
        &mut self,
        index: usize,
        delta: StreamDelta,
        on_delta: &mut impl FnMut(DeltaEvent),
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(block) = self.blocks.get_mut(index).and_then(Option::as_mut) else {
            return Err(Error::other(format!(
                "Anthropic stream referenced unknown content block index {index}"
            ))
            .into());
        };

        match (block, delta) {
            (StreamingContentBlock::Text(state), StreamDelta::Text { text }) => {
                let emitted = append_text_to_state(state, &text);
                if !emitted.is_empty() {
                    self.streamed_text = true;
                    on_delta(DeltaEvent::Text(emitted));
                }
            }
            (StreamingContentBlock::Thinking(state), StreamDelta::Thinking { thinking }) => {
                state.thinking.push_str(&thinking);
                if !thinking.is_empty() {
                    on_delta(DeltaEvent::Thinking(thinking));
                }
            }
            (StreamingContentBlock::Thinking(state), StreamDelta::Signature { signature }) => {
                state.signature.push_str(&signature);
            }
            (StreamingContentBlock::ToolUse(state), StreamDelta::InputJson { partial_json }) => {
                state.input_json.push_str(&partial_json);
            }
            _ => {}
        }

        Ok(())
    }
}

fn ensure_block_capacity(blocks: &mut Vec<Option<StreamingContentBlock>>, index: usize) {
    if blocks.len() <= index {
        blocks.resize_with(index + 1, || None);
    }
}

fn append_text_to_state(state: &mut TextBlockState, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    state.text.push_str(text);

    let mut emitted = String::new();
    if state.separator_pending {
        emitted.push('\n');
        state.separator_pending = false;
    }
    emitted.push_str(text);
    emitted
}

#[cfg(test)]
mod tests {
    use super::{DeltaEvent, SseParser, StreamingMessageBuilder};
    use crate::agent::types::ContentBlock;
    use serde_json::json;

    fn text_deltas(events: &[DeltaEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                DeltaEvent::Text(t) => Some(t.clone()),
                DeltaEvent::Thinking(_) => None,
            })
            .collect()
    }

    #[test]
    fn sse_parser_reassembles_split_events() {
        let mut parser = SseParser::default();
        let first = parser
            .push(
                b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,",
            )
            .unwrap();
        assert!(first.is_empty());

        let second = parser
            .push(b"\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n")
            .unwrap();

        assert_eq!(
            second,
            vec![
                "{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}"
                    .to_string()
            ]
        );
    }

    #[test]
    fn streaming_builder_collects_text_and_tool_use_blocks() {
        let mut builder = StreamingMessageBuilder::default();
        let mut events = Vec::new();

        builder
            .apply_event(
                "{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"Hello\"}}",
                &mut |e| events.push(e),
            )
            .unwrap();
        builder
            .apply_event(
                "{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}",
                &mut |e| events.push(e),
            )
            .unwrap();
        builder
            .apply_event(
                "{\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool-1\",\"name\":\"read_file\",\"input\":{}}}",
                &mut |e| events.push(e),
            )
            .unwrap();
        builder
            .apply_event(
                "{\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"src/main.rs\\\"}\"}}",
                &mut |e| events.push(e),
            )
            .unwrap();

        let response = builder.finish().unwrap();

        assert_eq!(
            text_deltas(&events),
            vec!["Hello".to_string(), " world".to_string()]
        );
        assert_eq!(
            response.content,
            vec![
                ContentBlock::Text {
                    text: "Hello world".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({ "path": "src/main.rs" }),
                },
            ]
        );
    }

    #[test]
    fn streaming_builder_inserts_separator_between_text_blocks() {
        let mut builder = StreamingMessageBuilder::default();
        let mut events = Vec::new();

        builder
            .apply_event(
                "{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"First\"}}",
                &mut |e| events.push(e),
            )
            .unwrap();
        builder
            .apply_event(
                "{\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"Second\"}}",
                &mut |e| events.push(e),
            )
            .unwrap();

        assert_eq!(
            text_deltas(&events),
            vec!["First".to_string(), "\nSecond".to_string()]
        );
    }
}
