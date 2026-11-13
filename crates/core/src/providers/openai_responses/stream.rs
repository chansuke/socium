//! Assembles a streamed Responses API reply into content blocks and the
//! same `ProviderEvent`s the other wires produce, so the turn loop and the
//! Activity pane do not depend on the wire.

use crate::agent::types::{AgentMessageResponse, ContentBlock};
use crate::providers::error::{ProviderError, describe_error};
use crate::providers::{BlockKind, ProviderEvent, ProviderResponse, TokenUsage};
use serde_json::{Value, json};

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

enum ItemKind {
    Message {
        text: String,
    },
    Reasoning {
        summary: String,
        summary_part: Option<u64>,
        /// The finished item as it is sent back, when it carries
        /// encrypted content.
        replay: Option<String>,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
}

struct ItemState {
    output_index: u64,
    block: usize,
    finished: bool,
    kind: ItemKind,
}

enum Outcome {
    Completed,
    Incomplete(String),
}

#[derive(Default)]
pub(super) struct ResponsesStreamBuilder {
    received_bytes: usize,
    next_block: usize,
    items: Vec<ItemState>,
    refusal: String,
    outcome: Option<Outcome>,
    usage: Option<TokenUsage>,
    streamed_text: bool,
}

impl ResponsesStreamBuilder {
    pub(super) fn apply_event(
        &mut self,
        data: &str,
        emit: &mut impl FnMut(ProviderEvent),
    ) -> Result<(), ProviderError> {
        self.received_bytes = self.received_bytes.saturating_add(data.len());
        if self.received_bytes > MAX_RESPONSE_BYTES {
            return Err(ProviderError::Stream("Response exceeded 8 MiB".into()));
        }
        let event: Value = serde_json::from_str(data)?;
        let output_index = event["output_index"].as_u64().unwrap_or(0);
        match event["type"].as_str().unwrap_or_default() {
            "response.output_item.added" => {
                self.item(output_index, &event["item"], emit);
            }
            "response.output_text.delta" => {
                let text = event["delta"].as_str().unwrap_or_default();
                self.push_text(output_index, text, emit);
            }
            "response.reasoning_summary_text.delta" => {
                let text = event["delta"].as_str().unwrap_or_default();
                let part = event["summary_index"].as_u64();
                let reasoning = json!({"type": "reasoning"});
                if let Some(state) = self.item(output_index, &reasoning, emit)
                    && let ItemKind::Reasoning {
                        summary,
                        summary_part,
                        ..
                    } = &mut state.kind
                {
                    let mut delta = String::new();
                    if !summary.is_empty() && part != *summary_part {
                        delta.push_str("\n\n");
                    }
                    delta.push_str(text);
                    *summary_part = part;
                    summary.push_str(&delta);
                    emit(ProviderEvent::ThinkingDelta {
                        index: state.block,
                        text: delta,
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                let fragment = event["delta"].as_str().unwrap_or_default();
                if let Some(state) = self.items.iter_mut().find(|state| {
                    state.output_index == output_index
                        && matches!(state.kind, ItemKind::FunctionCall { .. })
                }) && let ItemKind::FunctionCall { arguments, .. } = &mut state.kind
                    && !fragment.is_empty()
                {
                    arguments.push_str(fragment);
                    emit(ProviderEvent::ToolInputDelta {
                        index: state.block,
                        partial_json: fragment.to_string(),
                    });
                }
            }
            "response.refusal.delta" => {
                self.refusal
                    .push_str(event["delta"].as_str().unwrap_or_default());
            }
            "response.output_item.done" => self.finish_item(output_index, &event["item"], emit),
            "response.completed" => {
                self.usage = parse_usage(&event["response"]["usage"]);
                self.outcome = Some(Outcome::Completed);
            }
            "response.incomplete" => {
                let response = &event["response"];
                self.usage = parse_usage(&response["usage"]);
                let reason = response["incomplete_details"]["reason"]
                    .as_str()
                    .unwrap_or("unknown");
                self.outcome = Some(Outcome::Incomplete(reason.to_string()));
            }
            "response.failed" => {
                let detail = describe_error(&event["response"]["error"])
                    .unwrap_or_else(|| "unknown error".to_string());
                return Err(ProviderError::Stream(format!("Response failed: {detail}")));
            }
            "error" => {
                // The event itself carries code and message; its type is
                // just "error".
                let error = if event["error"].is_object() {
                    event["error"].clone()
                } else {
                    json!({"code": event["code"], "message": event["message"]})
                };
                let detail = describe_error(&error).unwrap_or_else(|| "unknown error".to_string());
                return Err(ProviderError::Stream(format!("Streaming error: {detail}")));
            }
            _ => {}
        }
        Ok(())
    }

    /// The state for the output item at `output_index`, started from `item`
    /// when it is new. Items other than messages, reasoning, and function
    /// calls have no block.
    fn item(
        &mut self,
        output_index: u64,
        item: &Value,
        emit: &mut impl FnMut(ProviderEvent),
    ) -> Option<&mut ItemState> {
        if let Some(position) = self
            .items
            .iter()
            .position(|state| state.output_index == output_index)
        {
            return Some(&mut self.items[position]);
        }
        let (kind, block_kind) = match item["type"].as_str()? {
            "message" => (
                ItemKind::Message {
                    text: String::new(),
                },
                BlockKind::Text,
            ),
            "reasoning" => (
                ItemKind::Reasoning {
                    summary: String::new(),
                    summary_part: None,
                    replay: None,
                },
                BlockKind::Thinking,
            ),
            "function_call" => {
                let call_id = item["call_id"].as_str().unwrap_or_default().to_string();
                let name = item["name"].as_str().unwrap_or_default().to_string();
                (
                    ItemKind::FunctionCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                    },
                    BlockKind::ToolUse { id: call_id, name },
                )
            }
            _ => return None,
        };
        let block = self.next_block;
        self.next_block += 1;
        emit(ProviderEvent::BlockStarted {
            index: block,
            kind: block_kind,
        });
        self.items.push(ItemState {
            output_index,
            block,
            finished: false,
            kind,
        });
        self.items.last_mut()
    }

    fn push_text(&mut self, output_index: u64, text: &str, emit: &mut impl FnMut(ProviderEvent)) {
        if text.is_empty() {
            return;
        }
        let message = json!({"type": "message"});
        if let Some(state) = self.item(output_index, &message, emit)
            && let ItemKind::Message { text: buffer } = &mut state.kind
        {
            buffer.push_str(text);
            let index = state.block;
            self.streamed_text = true;
            emit(ProviderEvent::TextDelta {
                index,
                text: text.to_string(),
            });
        }
    }

    /// Applies a finished item, which is authoritative over the deltas.
    fn finish_item(
        &mut self,
        output_index: u64,
        item: &Value,
        emit: &mut impl FnMut(ProviderEvent),
    ) {
        if item["type"] == "message" {
            let mut text = String::new();
            for part in item["content"].as_array().into_iter().flatten() {
                match part["type"].as_str() {
                    Some("output_text") => text.push_str(part["text"].as_str().unwrap_or_default()),
                    Some("refusal") if self.refusal.is_empty() => self
                        .refusal
                        .push_str(part["refusal"].as_str().unwrap_or_default()),
                    _ => {}
                }
            }
            let streamed = self
                .items
                .iter()
                .find(|state| state.output_index == output_index)
                .map_or(0, |state| match &state.kind {
                    ItemKind::Message { text } => text.len(),
                    _ => 0,
                });
            // Text that arrived only in the finished item still reaches the
            // transcript.
            if let Some(rest) = text.get(streamed..) {
                self.push_text(output_index, rest, emit);
            }
        }
        let Some(state) = self.item(output_index, item, emit) else {
            return;
        };
        match &mut state.kind {
            ItemKind::Message { .. } => {}
            ItemKind::Reasoning {
                summary, replay, ..
            } => {
                if summary.is_empty() {
                    *summary = summary_text(item);
                    if !summary.is_empty() {
                        emit(ProviderEvent::ThinkingDelta {
                            index: state.block,
                            text: summary.clone(),
                        });
                    }
                }
                if item["encrypted_content"].is_string() {
                    let mut kept = item.clone();
                    // Items are not stored with store: false, so an id
                    // would refer to nothing on the next request.
                    if let Some(object) = kept.as_object_mut() {
                        object.remove("id");
                        object.remove("status");
                    }
                    *replay = Some(kept.to_string());
                }
            }
            ItemKind::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                if let Some(whole) = item["arguments"].as_str() {
                    *arguments = whole.to_string();
                }
                if let Some(id) = item["call_id"].as_str() {
                    *call_id = id.to_string();
                }
                if let Some(item_name) = item["name"].as_str() {
                    *name = item_name.to_string();
                }
            }
        }
        if !state.finished {
            state.finished = true;
            emit(ProviderEvent::BlockFinished { index: state.block });
        }
    }

    /// Closes the open blocks, reports the stop reason in Anthropic's terms,
    /// and checks that the response completed safely.
    pub(super) fn finish(
        mut self,
        emit: &mut impl FnMut(ProviderEvent),
    ) -> Result<ProviderResponse, ProviderError> {
        let Some(outcome) = self.outcome.take() else {
            return Err(ProviderError::Stream(
                "Response ended before it completed".into(),
            ));
        };
        self.items.sort_by_key(|state| state.block);
        for state in self.items.iter_mut().filter(|state| !state.finished) {
            state.finished = true;
            emit(ProviderEvent::BlockFinished { index: state.block });
        }

        let has_tools = self
            .items
            .iter()
            .any(|state| matches!(state.kind, ItemKind::FunctionCall { .. }));
        let stop_reason = match &outcome {
            Outcome::Incomplete(reason) if reason == "max_output_tokens" => "max_tokens",
            Outcome::Incomplete(reason) => reason.as_str(),
            Outcome::Completed if !self.refusal.is_empty() => "refusal",
            Outcome::Completed if has_tools => "tool_use",
            Outcome::Completed => "end_turn",
        }
        .to_string();
        emit(ProviderEvent::MessageFinished {
            stop_reason: stop_reason.clone(),
            usage: self.usage,
        });
        match (&outcome, stop_reason.as_str()) {
            (Outcome::Completed, "tool_use" | "end_turn") => {}
            (Outcome::Completed, other) => {
                return Err(ProviderError::Stream(format!(
                    "Response stopped without safe completion (stop reason: {other})"
                )));
            }
            (Outcome::Incomplete(reason), "max_tokens") => {
                return Err(ProviderError::Stream(format!(
                    "Response stopped at the output token limit (incomplete reason: {reason}); \
                     raise max_output_tokens or MAX_TOKENS to allow longer responses"
                )));
            }
            (Outcome::Incomplete(reason), _) => {
                return Err(ProviderError::Stream(format!(
                    "Response stopped without safe completion (incomplete reason: {reason})"
                )));
            }
        }

        let mut blocks = Vec::with_capacity(self.items.len());
        for state in self.items {
            match state.kind {
                ItemKind::Message { text } if !text.is_empty() => {
                    blocks.push(ContentBlock::Text { text });
                }
                ItemKind::Message { .. } | ItemKind::Reasoning { replay: None, .. } => {}
                ItemKind::Reasoning {
                    summary,
                    replay: Some(replay),
                    ..
                } => blocks.push(ContentBlock::Thinking {
                    thinking: summary,
                    signature: replay,
                }),
                ItemKind::FunctionCall {
                    call_id,
                    name,
                    arguments,
                } => {
                    let input = if arguments.trim().is_empty() {
                        Value::Object(Default::default())
                    } else {
                        serde_json::from_str(&arguments).map_err(|_| {
                            ProviderError::Stream(format!(
                                "Arguments of the {name} tool call are not valid JSON"
                            ))
                        })?
                    };
                    blocks.push(ContentBlock::ToolUse {
                        id: call_id,
                        name,
                        input,
                    });
                }
            }
        }
        Ok(ProviderResponse {
            message: AgentMessageResponse { content: blocks },
            streamed_text: self.streamed_text,
            usage: self.usage,
        })
    }
}

fn summary_text(item: &Value) -> String {
    item["summary"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Responses usage: `input_tokens` includes cached tokens and tokens newly
/// written to the cache, which are reported as cache reads and writes.
fn parse_usage(usage: &Value) -> Option<TokenUsage> {
    let to_u32 = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
    let input = usage["input_tokens"].as_u64()?;
    let details = &usage["input_tokens_details"];
    let cached = details["cached_tokens"].as_u64().unwrap_or(0).min(input);
    let written = details["cache_write_tokens"]
        .as_u64()
        .unwrap_or(0)
        .min(input - cached);
    Some(TokenUsage {
        input_tokens: to_u32(input - cached - written),
        output_tokens: to_u32(usage["output_tokens"].as_u64().unwrap_or(0)),
        cache_creation_input_tokens: to_u32(written),
        cache_read_input_tokens: to_u32(cached),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(events: &[Value]) -> (Result<ProviderResponse, ProviderError>, Vec<ProviderEvent>) {
        let mut emitted = Vec::new();
        let mut builder = ResponsesStreamBuilder::default();
        let mut emit = |event| emitted.push(event);
        for event in events {
            if let Err(err) = builder.apply_event(&event.to_string(), &mut emit) {
                return (Err(err), emitted);
            }
        }
        let result = builder.finish(&mut emit);
        (result, emitted)
    }

    fn completed() -> Value {
        json!({"type": "response.completed", "response": {"status": "completed", "usage": {
            "input_tokens": 120,
            "input_tokens_details": {"cached_tokens": 100},
            "output_tokens": 30,
            "output_tokens_details": {"reasoning_tokens": 10}
        }}})
    }

    fn incomplete(reason: &str) -> Value {
        json!({"type": "response.incomplete", "response": {
            "status": "incomplete",
            "incomplete_details": {"reason": reason}
        }})
    }

    fn message_events(output_index: u64, text: &str) -> Vec<Value> {
        vec![
            json!({"type": "response.output_item.added", "output_index": output_index,
                "item": {"type": "message", "id": "msg_1", "role": "assistant", "content": []}}),
            json!({"type": "response.output_text.delta", "output_index": output_index,
                "item_id": "msg_1", "content_index": 0, "delta": text}),
            json!({"type": "response.output_item.done", "output_index": output_index,
                "item": {"type": "message", "id": "msg_1", "role": "assistant",
                    "content": [{"type": "output_text", "text": text}]}}),
        ]
    }

    fn reasoning_item(encrypted: Option<&str>) -> Value {
        let mut item = json!({
            "type": "reasoning",
            "id": "rs_1",
            "status": "completed",
            "summary": [
                {"type": "summary_text", "text": "Plan"},
                {"type": "summary_text", "text": "Read"}
            ],
        });
        if let Some(encrypted) = encrypted {
            item["encrypted_content"] = json!(encrypted);
        }
        item
    }

    fn reasoning_events(encrypted: Option<&str>) -> Vec<Value> {
        vec![
            json!({"type": "response.output_item.added", "output_index": 0,
                "item": {"type": "reasoning", "id": "rs_1", "summary": []}}),
            json!({"type": "response.reasoning_summary_text.delta", "output_index": 0,
                "item_id": "rs_1", "summary_index": 0, "delta": "Plan"}),
            json!({"type": "response.reasoning_summary_text.delta", "output_index": 0,
                "item_id": "rs_1", "summary_index": 1, "delta": "Read"}),
            json!({"type": "response.output_item.done", "output_index": 0,
                "item": reasoning_item(encrypted)}),
        ]
    }

    fn function_call_events(output_index: u64, arguments: &str) -> Vec<Value> {
        let (head, tail) = arguments.split_at(arguments.len() / 2);
        vec![
            json!({"type": "response.output_item.added", "output_index": output_index,
                "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1",
                    "name": "read_file", "arguments": ""}}),
            json!({"type": "response.function_call_arguments.delta",
                "output_index": output_index, "item_id": "fc_1", "delta": head}),
            json!({"type": "response.function_call_arguments.delta",
                "output_index": output_index, "item_id": "fc_1", "delta": tail}),
            json!({"type": "response.output_item.done", "output_index": output_index,
                "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1",
                    "name": "read_file", "arguments": arguments, "status": "completed"}}),
        ]
    }

    #[test]
    fn text_streams_and_ends_the_turn_with_usage() {
        let mut events = vec![json!({"type": "response.created", "response": {}})];
        events.extend(message_events(0, "Hello"));
        events.push(completed());
        let (result, emitted) = run(&events);
        let response = result.unwrap();
        assert_eq!(
            response.message.content,
            [ContentBlock::Text {
                text: "Hello".into()
            }]
        );
        assert!(response.streamed_text);
        let usage = TokenUsage {
            input_tokens: 20,
            output_tokens: 30,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 100,
        };
        assert_eq!(response.usage, Some(usage));
        assert_eq!(
            emitted,
            [
                ProviderEvent::BlockStarted {
                    index: 0,
                    kind: BlockKind::Text
                },
                ProviderEvent::TextDelta {
                    index: 0,
                    text: "Hello".into()
                },
                ProviderEvent::BlockFinished { index: 0 },
                ProviderEvent::MessageFinished {
                    stop_reason: "end_turn".into(),
                    usage: Some(usage)
                },
            ]
        );
    }

    #[test]
    fn cache_writes_are_reported_apart_from_live_input() {
        // Shape observed from gpt-6-astra on 2026-11-14: input_tokens
        // includes both cached and newly written tokens.
        let usage = parse_usage(&json!({
            "input_tokens": 1406,
            "input_tokens_details": {"cached_tokens": 1074, "cache_write_tokens": 329},
            "output_tokens": 23,
        }));
        assert_eq!(
            usage,
            Some(TokenUsage {
                input_tokens: 3,
                output_tokens: 23,
                cache_creation_input_tokens: 329,
                cache_read_input_tokens: 1074,
            })
        );
    }

    #[test]
    fn reasoning_is_kept_for_replay_and_a_tool_call_ends_the_turn() {
        let mut events = reasoning_events(Some("opaque"));
        events.extend(function_call_events(1, r#"{"path":"a.rs"}"#));
        events.push(completed());
        let (result, emitted) = run(&events);
        let response = result.unwrap();
        let [
            ContentBlock::Thinking {
                thinking,
                signature,
            },
            ContentBlock::ToolUse { id, name, input },
        ] = response.message.content.as_slice()
        else {
            panic!("{:?}", response.message.content);
        };
        assert_eq!(thinking, "Plan\n\nRead");
        // Items are not stored with store: false, so the id is not sent back.
        let replay: Value = serde_json::from_str(signature).unwrap();
        assert_eq!(
            replay,
            json!({
                "type": "reasoning",
                "summary": [
                    {"type": "summary_text", "text": "Plan"},
                    {"type": "summary_text", "text": "Read"}
                ],
                "encrypted_content": "opaque",
            })
        );
        assert_eq!((id.as_str(), name.as_str()), ("call_1", "read_file"));
        assert_eq!(input, &json!({"path": "a.rs"}));

        let thinking_text = emitted
            .iter()
            .filter_map(|event| match event {
                ProviderEvent::ThinkingDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(thinking_text, "Plan\n\nRead");
        assert!(emitted.contains(&ProviderEvent::BlockStarted {
            index: 1,
            kind: BlockKind::ToolUse {
                id: "call_1".into(),
                name: "read_file".into()
            }
        }));
        assert!(emitted.contains(&ProviderEvent::ToolInputDelta {
            index: 1,
            partial_json: r#"{"path""#.into()
        }));
        assert!(matches!(
            emitted.last(),
            Some(ProviderEvent::MessageFinished { stop_reason, .. }) if stop_reason == "tool_use"
        ));
    }

    #[test]
    fn reasoning_without_encrypted_content_is_shown_but_not_kept() {
        let mut events = reasoning_events(None);
        events.extend(message_events(1, "Done"));
        events.push(completed());
        let (result, emitted) = run(&events);
        assert_eq!(
            result.unwrap().message.content,
            [ContentBlock::Text {
                text: "Done".into()
            }]
        );
        assert!(
            emitted
                .iter()
                .any(|event| matches!(event, ProviderEvent::ThinkingDelta { .. }))
        );
    }

    #[test]
    fn the_finished_item_is_authoritative_for_arguments_and_text() {
        let events = [
            json!({"type": "response.output_item.added", "output_index": 0,
                "item": {"type": "function_call", "call_id": "call_9", "name": "read_file"}}),
            json!({"type": "response.output_item.done", "output_index": 0,
                "item": {"type": "function_call", "call_id": "call_9", "name": "read_file",
                    "arguments": "{\"path\":\"b.rs\"}"}}),
            json!({"type": "response.output_item.done", "output_index": 1,
                "item": {"type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": "Reading b.rs."}]}}),
            completed(),
        ];
        let (result, emitted) = run(&events);
        assert_eq!(
            result.unwrap().message.content,
            [
                ContentBlock::ToolUse {
                    id: "call_9".into(),
                    name: "read_file".into(),
                    input: json!({"path": "b.rs"}),
                },
                ContentBlock::Text {
                    text: "Reading b.rs.".into()
                },
            ]
        );
        assert!(emitted.contains(&ProviderEvent::TextDelta {
            index: 1,
            text: "Reading b.rs.".into()
        }));
    }

    #[test]
    fn the_output_token_limit_is_reported_with_its_reason() {
        let mut events = message_events(0, "partial");
        events.push(incomplete("max_output_tokens"));
        let (result, emitted) = run(&events);
        let err = result.err().unwrap().to_string();
        assert!(err.contains("output token limit"), "{err}");
        assert!(err.contains("max_output_tokens"), "{err}");
        assert!(matches!(
            emitted.last(),
            Some(ProviderEvent::MessageFinished { stop_reason, .. }) if stop_reason == "max_tokens"
        ));
    }

    #[test]
    fn other_incomplete_reasons_and_refusals_are_unsafe_stops() {
        let mut filtered = message_events(0, "partial");
        filtered.push(incomplete("content_filter"));
        let err = run(&filtered).0.err().unwrap().to_string();
        assert_eq!(
            err,
            "Response stopped without safe completion (incomplete reason: content_filter)"
        );

        let refused = [
            json!({"type": "response.output_item.added", "output_index": 0,
                "item": {"type": "message", "role": "assistant", "content": []}}),
            json!({"type": "response.refusal.delta", "output_index": 0, "delta": "I can't"}),
            json!({"type": "response.output_item.done", "output_index": 0,
                "item": {"type": "message", "role": "assistant",
                    "content": [{"type": "refusal", "refusal": "I can't"}]}}),
            completed(),
        ];
        let (result, emitted) = run(&refused);
        let err = result.err().unwrap().to_string();
        assert_eq!(
            err,
            "Response stopped without safe completion (stop reason: refusal)"
        );
        assert!(matches!(
            emitted.last(),
            Some(ProviderEvent::MessageFinished { stop_reason, .. }) if stop_reason == "refusal"
        ));
    }

    #[test]
    fn failures_and_error_events_show_the_api_message() {
        let failed = [json!({"type": "response.failed", "response": {
            "status": "failed",
            "error": {"code": "server_error", "message": "The model crashed"}
        }})];
        let err = run(&failed).0.err().unwrap().to_string();
        assert!(err.contains("server_error: The model crashed"), "{err}");

        let error = [json!({"type": "error", "code": "rate_limit_exceeded",
            "message": "Slow down", "param": null})];
        let err = run(&error).0.err().unwrap().to_string();
        assert!(err.contains("rate_limit_exceeded: Slow down"), "{err}");
    }

    #[test]
    fn a_stream_cut_before_completion_fails() {
        let err = run(&message_events(0, "partial"))
            .0
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("before it completed"), "{err}");
    }

    #[test]
    fn invalid_arguments_fail_the_response() {
        let mut events = function_call_events(0, r#"{"path":"#);
        events.push(completed());
        let err = run(&events).0.err().unwrap().to_string();
        assert!(err.contains("read_file"), "{err}");
    }
}
