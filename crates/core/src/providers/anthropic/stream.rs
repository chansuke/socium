use crate::agent::types::{AgentMessageResponse, ContentBlock};
use crate::providers::error::ProviderError;
use crate::providers::{BlockKind, ProviderEvent, TokenUsage};
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
    /// An event type added after this parser was written. The API's
    /// versioning policy allows new types; they are skipped, and the block
    /// and stop checks below still decide whether the response is complete.
    #[serde(other)]
    Unknown,
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
    /// Arrives whole in `content_block_start`; no deltas follow.
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    /// A block type this parser does not know, such as a server tool
    /// result. Its deltas are skipped and it is left out of the history.
    #[serde(other)]
    Unknown,
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
    /// A delta type added later, such as citations; skipped.
    #[serde(other)]
    Unknown,
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
    RedactedThinking(String),
    ToolUse(ToolUseBlockState),
    Ignored,
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
        on_delta: &mut impl FnMut(ProviderEvent),
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
                on_delta(ProviderEvent::BlockFinished { index });
            }
            AnthropicStreamEvent::MessageStop => {
                let Some(stop_reason) = self.stop_reason.clone() else {
                    return Err(ProviderError::Stream("Incomplete message_stop".into()));
                };
                if self.active.is_some() {
                    return Err(ProviderError::Stream("Incomplete message_stop".into()));
                }
                self.stopped = true;
                on_delta(ProviderEvent::MessageFinished {
                    stop_reason,
                    usage: self.usage,
                });
            }
            AnthropicStreamEvent::Ping | AnthropicStreamEvent::Unknown => {}
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
        crate::providers::stop_reason_allows_release(self.stop_reason.as_deref(), has_tools)?;
        let mut content = Vec::with_capacity(self.blocks.len());
        let mut invalid_tool_inputs = Vec::new();
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
                StreamingContentBlock::RedactedThinking(data) => {
                    ContentBlock::RedactedThinking { data }
                }
                StreamingContentBlock::ToolUse(state) => {
                    let input = if state.input_json.trim().is_empty() {
                        state.initial_input
                    } else {
                        // Eager streaming skips the API's own validation.
                        // A malformed input is reported back to the model
                        // as that call's error instead of failing the turn.
                        match serde_json::from_str::<Value>(&state.input_json) {
                            Ok(input) if input.is_object() => input,
                            Ok(_) => {
                                invalid_tool_inputs.push((
                                    state.id.clone(),
                                    "the input is not a JSON object".into(),
                                ));
                                Value::Object(Default::default())
                            }
                            Err(err) => {
                                invalid_tool_inputs.push((state.id.clone(), err.to_string()));
                                Value::Object(Default::default())
                            }
                        }
                    };
                    ContentBlock::ToolUse {
                        id: state.id,
                        name: state.name,
                        input,
                    }
                }
                StreamingContentBlock::Ignored => continue,
            });
        }
        Ok(AgentMessageResponse {
            content,
            stop_reason: self.stop_reason,
            invalid_tool_inputs,
        })
    }

    fn start_block(
        &mut self,
        index: usize,
        content_block: StreamContentBlockStart,
        on_delta: &mut impl FnMut(ProviderEvent),
    ) {
        ensure_block_capacity(&mut self.blocks, index);
        let kind = match &content_block {
            StreamContentBlockStart::Text { .. } => Some(BlockKind::Text),
            StreamContentBlockStart::Thinking { .. }
            | StreamContentBlockStart::RedactedThinking { .. } => Some(BlockKind::Thinking),
            StreamContentBlockStart::ToolUse { id, name, .. } => Some(BlockKind::ToolUse {
                id: id.clone(),
                name: name.clone(),
            }),
            StreamContentBlockStart::Unknown => None,
        };
        if let Some(kind) = kind {
            on_delta(ProviderEvent::BlockStarted { index, kind });
        }
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
                        on_delta(ProviderEvent::TextDelta {
                            index,
                            text: emitted,
                        });
                    }
                }
                StreamingContentBlock::Text(state)
            }
            StreamContentBlockStart::Thinking { thinking } => {
                if !thinking.is_empty() {
                    on_delta(ProviderEvent::ThinkingDelta {
                        index,
                        text: thinking.clone(),
                    });
                }
                StreamingContentBlock::Thinking(ThinkingBlockState {
                    thinking,
                    signature: String::new(),
                })
            }
            // The encrypted data is kept for the next request and never
            // emitted as thinking text.
            StreamContentBlockStart::RedactedThinking { data } => {
                StreamingContentBlock::RedactedThinking(data)
            }
            StreamContentBlockStart::ToolUse { id, name, input } => {
                StreamingContentBlock::ToolUse(ToolUseBlockState {
                    id,
                    name,
                    initial_input: input,
                    input_json: String::new(),
                })
            }
            StreamContentBlockStart::Unknown => StreamingContentBlock::Ignored,
        };
        self.blocks[index] = Some(block);
    }

    fn apply_delta(
        &mut self,
        index: usize,
        delta: StreamDelta,
        on_delta: &mut impl FnMut(ProviderEvent),
    ) -> Result<(), ProviderError> {
        let Some(block) = self.blocks.get_mut(index).and_then(Option::as_mut) else {
            return Err(ProviderError::Stream(format!(
                "Anthropic stream referenced unknown content block index {index}"
            )));
        };

        match (block, delta) {
            (StreamingContentBlock::Ignored, _) | (_, StreamDelta::Unknown) => {}
            (StreamingContentBlock::Text(state), StreamDelta::Text { text }) => {
                let emitted = append_text_to_state(state, &text);
                if !emitted.is_empty() {
                    self.streamed_text = true;
                    on_delta(ProviderEvent::TextDelta {
                        index,
                        text: emitted,
                    });
                }
            }
            (StreamingContentBlock::Thinking(state), StreamDelta::Thinking { thinking }) => {
                state.thinking.push_str(&thinking);
                if !thinking.is_empty() {
                    on_delta(ProviderEvent::ThinkingDelta {
                        index,
                        text: thinking,
                    });
                }
            }
            (StreamingContentBlock::Thinking(state), StreamDelta::Signature { signature }) => {
                state.signature.push_str(&signature);
            }
            (StreamingContentBlock::ToolUse(state), StreamDelta::InputJson { partial_json }) => {
                state.input_json.push_str(&partial_json);
                if !partial_json.is_empty() {
                    on_delta(ProviderEvent::ToolInputDelta {
                        index,
                        partial_json,
                    });
                }
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

/// The raw event as the Activity pane may show it. The encrypted `data` of
/// a redacted thinking block is replaced by its size; the parser still gets
/// the original event.
pub(super) fn displayable_raw_event(raw_event: &str) -> String {
    let Ok(mut event) = serde_json::from_str::<Value>(raw_event) else {
        return raw_event.to_string();
    };
    let block = &mut event["content_block"];
    if block["type"] != "redacted_thinking" {
        return raw_event.to_string();
    }
    let size = block["data"].as_str().map_or(0, str::len);
    block["data"] = Value::String(format!("[encrypted, {size} bytes]"));
    event.to_string()
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
    fn unsafe_stops_are_reported_without_debug_formatting() {
        let finish_with = |reason: &str| {
            let mut b = start();
            tool(&mut b);
            stop(&mut b, reason);
            b.finish().unwrap_err().to_string()
        };
        assert_eq!(
            finish_with("max_tokens"),
            "Response stopped at the output token limit (stop reason: max_tokens) while writing a tool call; raise MAX_TOKENS or the model's max_output_tokens"
        );
        assert!(finish_with("refusal").contains("declined"));
        assert_eq!(
            finish_with("end_turn"),
            "Response stopped without safe completion (stop reason: end_turn)"
        );
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
    fn tool_input_fragments_stream_but_only_complete_json_is_released() {
        let mut b = start();
        let mut deltas = Vec::new();
        for event in [
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t1","name":"read_file","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"a.rs\""}}),
        ] {
            b.apply_event(&event.to_string(), &mut |d| deltas.push(d))
                .unwrap();
        }
        assert_eq!(
            deltas,
            vec![
                ProviderEvent::BlockStarted {
                    index: 0,
                    kind: BlockKind::ToolUse {
                        id: "t1".into(),
                        name: "read_file".into()
                    }
                },
                ProviderEvent::ToolInputDelta {
                    index: 0,
                    partial_json: "{\"path\":".into()
                },
                ProviderEvent::ToolInputDelta {
                    index: 0,
                    partial_json: "\"a.rs\"".into()
                },
            ]
        );
        // The model stopped mid-object: the fragments were shown, but the
        // call carries no input and is marked so the turn never runs it.
        stop(&mut b, "tool_use");
        let response = b.finish().unwrap();
        assert!(matches!(
            &response.content[0],
            ContentBlock::ToolUse { input, .. } if *input == json!({})
        ));
        assert_eq!(response.invalid_tool_inputs[0].0, "t1");
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
        assert_eq!(
            deltas,
            vec![
                ProviderEvent::BlockStarted {
                    index: 0,
                    kind: BlockKind::Thinking
                },
                ProviderEvent::ThinkingDelta {
                    index: 0,
                    text: "reason".into()
                },
                ProviderEvent::BlockFinished { index: 0 },
                ProviderEvent::BlockStarted {
                    index: 1,
                    kind: BlockKind::Text
                },
                ProviderEvent::TextDelta {
                    index: 1,
                    text: "answer".into()
                },
                ProviderEvent::BlockFinished { index: 1 },
                ProviderEvent::MessageFinished {
                    stop_reason: "end_turn".into(),
                    usage: Some(TokenUsage {
                        input_tokens: 12,
                        ..TokenUsage::default()
                    })
                },
            ]
        );
        assert!(
            matches!(&b.finish().unwrap().content[0],ContentBlock::Thinking{signature,..} if signature=="sig")
        );
    }

    #[test]
    fn unknown_events_blocks_and_deltas_are_skipped() {
        let mut b = start();
        for event in [
            json!({"type": "some_future_event", "detail": {}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_result","data":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"future_delta","x":1}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":"hi"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"citations_delta","citation":{}}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
            json!({"type":"message_stop"}),
        ] {
            apply(&mut b, event).unwrap();
        }
        assert_eq!(
            b.finish().unwrap().content,
            [ContentBlock::Text { text: "hi".into() }]
        );
    }

    #[test]
    fn a_text_reply_cut_at_the_limit_is_kept_and_marked() {
        let mut b = start();
        apply(&mut b, json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"long answer"}})).unwrap();
        stop(&mut b, "max_tokens");
        let response = b.finish().unwrap();
        assert_eq!(response.stop_reason.as_deref(), Some("max_tokens"));
        assert_eq!(response.content.len(), 1);
    }

    #[test]
    fn raw_events_shown_in_activity_hide_redacted_data() {
        let raw = json!({"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"c2VjcmV0"}}).to_string();
        let shown = displayable_raw_event(&raw);
        assert!(!shown.contains("c2VjcmV0"), "{shown}");
        assert!(shown.contains("[encrypted, 8 bytes]"));
        let text = json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}).to_string();
        assert_eq!(displayable_raw_event(&text), text);
    }

    #[test]
    fn redacted_thinking_is_kept_opaque_beside_a_tool_call_and_never_streamed() {
        let data = "EmwKAhgBEgy3va3pzix/LafPsn4aDFIT2Xlxh0L5L8rLVyIwxtE3rAFBa8cr3qpP";
        let mut b = start();
        let mut deltas = Vec::new();
        for event in [
            json!({"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":data}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"read_file","input":{"path":"a.rs"}}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
            json!({"type":"message_stop"}),
        ] {
            b.apply_event(&event.to_string(), &mut |d| deltas.push(d))
                .unwrap();
        }
        assert!(!deltas.iter().any(|d| matches!(
            d,
            ProviderEvent::ThinkingDelta { .. } | ProviderEvent::TextDelta { .. }
        )));
        let content = b.finish().unwrap().content;
        assert_eq!(
            content[0],
            ContentBlock::RedactedThinking { data: data.into() }
        );
        assert!(matches!(&content[1], ContentBlock::ToolUse { id, .. } if id == "t1"));
    }
}
