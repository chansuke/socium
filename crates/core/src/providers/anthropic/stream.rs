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
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        #[serde(default)]
        delta: Option<StopDelta>,
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
struct StopDelta {
    stop_reason: Option<String>,
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
    received_bytes: usize,
    started: bool,
    stopped: bool,
    active: Option<usize>,
    stop_reason: Option<String>,
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
        self.received_bytes = self.received_bytes.saturating_add(raw_event.len());
        if self.received_bytes > 8 * 1024 * 1024 {
            return Err(ProviderError::Stream("Response exceeded 8 MiB".into()));
        }
        let event: AnthropicStreamEvent = serde_json::from_str(raw_event)?;
        if self.stopped && !matches!(event, AnthropicStreamEvent::Ping) {
            return Err(ProviderError::Stream("Event after message_stop".into()));
        }
        if !self.started
            && !matches!(
                event,
                AnthropicStreamEvent::MessageStart { .. }
                    | AnthropicStreamEvent::Ping
                    | AnthropicStreamEvent::Error { .. }
            )
        {
            return Err(ProviderError::Stream("Missing message_start".into()));
        }
        match event {
            AnthropicStreamEvent::MessageStart { message } => {
                if self.started {
                    return Err(ProviderError::Stream("Duplicate message_start".into()));
                }
                self.started = true;
                if let Some(usage) = message.and_then(|m| m.usage) {
                    self.merge_usage(usage);
                }
            }
            AnthropicStreamEvent::MessageDelta { usage, delta } => {
                if self.active.is_some() {
                    return Err(ProviderError::Stream(
                        "message_delta before content_block_stop".into(),
                    ));
                }
                if let Some(reason) = delta.and_then(|d| d.stop_reason) {
                    self.stop_reason = Some(reason);
                }
                if let Some(usage) = usage {
                    self.merge_usage(usage);
                }
            }
            AnthropicStreamEvent::ContentBlockStop { index } => {
                if self.active != Some(index) {
                    return Err(ProviderError::Stream(
                        "Invalid content_block_stop index".into(),
                    ));
                }
                self.active = None;
            }
            AnthropicStreamEvent::MessageStop => {
                if self.active.is_some() || self.stop_reason.is_none() {
                    return Err(ProviderError::Stream("Incomplete message_stop".into()));
                }
                self.stopped = true;
            }
            AnthropicStreamEvent::Ping => {}
            AnthropicStreamEvent::Error { error } => {
                return Err(ProviderError::Stream(format!(
                    "Anthropic streaming error: {}",
                    error.message
                )));
            }
            AnthropicStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                if self.active.is_some() || index != self.blocks.len() || self.stop_reason.is_some()
                {
                    return Err(ProviderError::Stream(
                        "Invalid content_block_start sequence".into(),
                    ));
                }
                self.active = Some(index);
                self.start_block(index, content_block, on_delta);
            }
            AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                if self.active != Some(index) {
                    return Err(ProviderError::Stream("Delta for inactive block".into()));
                }
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
        if !self.started || !self.stopped {
            return Err(ProviderError::Stream(
                "Incomplete response: missing message_stop".into(),
            ));
        }
        let has_tools = self
            .blocks
            .iter()
            .any(|b| matches!(b, Some(StreamingContentBlock::ToolUse(_))));
        match self.stop_reason.as_deref() {
            Some("tool_use") if has_tools => {}
            Some("end_turn" | "stop_sequence") if !has_tools => {}
            reason => {
                return Err(ProviderError::Stream(format!(
                    "Response stopped without safe completion: {reason:?}"
                )));
            }
        }
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
            _ => {
                return Err(ProviderError::Stream(
                    "Delta type does not match content block".into(),
                ));
            }
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
    use super::*;
    use serde_json::json;
    fn apply(b: &mut StreamingMessageBuilder, e: Value) -> Result<(), ProviderError> {
        b.apply_event(&e.to_string(), &mut |_| {})
    }
    fn start() -> StreamingMessageBuilder {
        let mut b = StreamingMessageBuilder::default();
        apply(
            &mut b,
            json!({"type":"message_start", "message":{"usage":{"input_tokens":12}}}),
        )
        .unwrap();
        b
    }
    fn tool(b: &mut StreamingMessageBuilder) {
        apply(b, json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"apply_patch","input":{}}})).unwrap();
        apply(b, json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"patch\":\"test\"}"}})).unwrap();
    }
    fn stop(b: &mut StreamingMessageBuilder, reason: &str) {
        apply(b, json!({"type":"content_block_stop","index":0})).unwrap();
        apply(b,json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":7}})).unwrap();
        apply(b, json!({"type":"message_stop"})).unwrap();
    }
    #[test]
    fn complete_tool_and_usage_are_restored() {
        let mut b = start();
        tool(&mut b);
        stop(&mut b, "tool_use");
        assert_eq!(b.usage.as_ref().unwrap().output_tokens, 7);
        assert_eq!(b.usage.as_ref().unwrap().input_tokens, 12);
        assert!(
            matches!(&b.finish().unwrap().content[0], ContentBlock::ToolUse {input,..} if input["patch"] == "test")
        );
    }
    #[test]
    fn eof_and_token_limit_never_release_tool_calls() {
        let mut b = start();
        tool(&mut b);
        assert!(b.finish().is_err());
        let mut b = start();
        tool(&mut b);
        stop(&mut b, "max_tokens");
        assert!(b.finish().unwrap_err().to_string().contains("max_tokens"));
    }
    #[test]
    fn rejects_unbounded_indices_duplicate_blocks_and_wrong_deltas() {
        let mut b = start();
        assert!(apply(&mut b,json!({"type":"content_block_start","index":999999999,"content_block":{"type":"text","text":"x"}})).is_err());
        tool(&mut b);
        assert!(apply(&mut b,json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"bad"}})).is_err());
        assert!(apply(&mut b, json!({"type":"content_block_stop","index":1})).is_err());
    }
    #[test]
    fn text_and_thinking_stream_normally() {
        let mut b = start();
        let mut deltas = Vec::new();
        for event in [
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"reason"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":"answer"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
            json!({"type":"message_stop"}),
        ] {
            b.apply_event(&event.to_string(), &mut |d| deltas.push(d))
                .unwrap();
        }
        assert!(b.streamed_text);
        assert_eq!(deltas.len(), 2);
        assert!(
            matches!(&b.finish().unwrap().content[0],ContentBlock::Thinking{signature,..} if signature=="sig")
        );
    }
}
