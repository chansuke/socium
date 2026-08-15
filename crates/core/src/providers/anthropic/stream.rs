use crate::agent::types::{AgentMessageResponse, ContentBlock};
use crate::providers::error::ProviderError;
use crate::providers::{DeltaEvent, TokenUsage};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart {
        #[serde(default)]
        message: Option<StreamMessageStart>,
    },
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
    MessageDelta {
        #[serde(default)]
        usage: Option<StreamUsage>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: AnthropicErrorPayload },
}

#[derive(Debug, Deserialize)]
struct StreamMessageStart {
    #[serde(default)]
    usage: Option<StreamUsage>,
}

/// Usage fragment carried by `message_start` (input side) and
/// `message_delta` (cumulative output side). Fields are optional because
/// each event only reports the counters it knows about.
#[derive(Debug, Deserialize)]
struct StreamUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
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
pub(super) struct StreamingMessageBuilder {
    blocks: Vec<Option<StreamingContentBlock>>,
    text_block_count: usize,
    pub(super) streamed_text: bool,
    pub(super) usage: Option<TokenUsage>,
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
    pub(super) fn apply_event(
        &mut self,
        raw_event: &str,
        on_delta: &mut impl FnMut(DeltaEvent),
    ) -> Result<(), ProviderError> {
        let event: AnthropicStreamEvent = serde_json::from_str(raw_event)?;
        match event {
            AnthropicStreamEvent::MessageStart { message } => {
                if let Some(usage) = message.and_then(|m| m.usage) {
                    self.merge_usage(usage);
                }
            }
            AnthropicStreamEvent::MessageDelta { usage } => {
                if let Some(usage) = usage {
                    self.merge_usage(usage);
                }
            }
            AnthropicStreamEvent::ContentBlockStop
            | AnthropicStreamEvent::MessageStop
            | AnthropicStreamEvent::Ping => {}
            AnthropicStreamEvent::Error { error } => {
                return Err(ProviderError::Stream(format!(
                    "Anthropic streaming error: {}",
                    error.message
                )));
            }
            AnthropicStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => self.start_block(index, content_block, on_delta),
            AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                self.apply_delta(index, delta, on_delta)?
            }
        }
        Ok(())
    }

    /// Counters arrive incrementally: `message_start` carries the input
    /// side, `message_delta` carries the cumulative output side, so each
    /// present field overwrites rather than accumulates.
    fn merge_usage(&mut self, incoming: StreamUsage) {
        let usage = self.usage.get_or_insert_with(TokenUsage::default);
        if let Some(value) = incoming.input_tokens {
            usage.input_tokens = value;
        }
        if let Some(value) = incoming.output_tokens {
            usage.output_tokens = value;
        }
        if let Some(value) = incoming.cache_creation_input_tokens {
            usage.cache_creation_input_tokens = value;
        }
        if let Some(value) = incoming.cache_read_input_tokens {
            usage.cache_read_input_tokens = value;
        }
    }

    pub(super) fn finish(self) -> Result<AgentMessageResponse, ProviderError> {
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
    ) {
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
    }

    fn apply_delta(
        &mut self,
        index: usize,
        delta: StreamDelta,
        on_delta: &mut impl FnMut(DeltaEvent),
    ) -> Result<(), ProviderError> {
        let Some(block) = self.blocks.get_mut(index).and_then(Option::as_mut) else {
            return Err(ProviderError::Stream(format!(
                "Anthropic stream referenced unknown content block index {index}"
            )));
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
    use super::StreamingMessageBuilder;
    use crate::agent::types::ContentBlock;
    use crate::providers::DeltaEvent;
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
    fn streaming_builder_collects_usage_from_message_start_and_delta() {
        let mut builder = StreamingMessageBuilder::default();
        let mut sink = |_e| {};

        builder
            .apply_event(
                "{\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":1200,\"output_tokens\":1,\"cache_creation_input_tokens\":300,\"cache_read_input_tokens\":4500}}}",
                &mut sink,
            )
            .unwrap();
        builder
            .apply_event(
                "{\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":87}}",
                &mut sink,
            )
            .unwrap();

        let usage = builder.usage.expect("usage should be captured");
        assert_eq!(usage.input_tokens, 1200);
        assert_eq!(usage.output_tokens, 87);
        assert_eq!(usage.cache_creation_input_tokens, 300);
        assert_eq!(usage.cache_read_input_tokens, 4500);
        assert_eq!(usage.total_input_tokens(), 6000);
    }

    #[test]
    fn streaming_builder_reports_no_usage_when_events_omit_it() {
        let mut builder = StreamingMessageBuilder::default();
        let mut sink = |_e| {};

        builder
            .apply_event("{\"type\":\"message_start\"}", &mut sink)
            .unwrap();
        builder
            .apply_event("{\"type\":\"message_delta\"}", &mut sink)
            .unwrap();

        assert!(builder.usage.is_none());
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
