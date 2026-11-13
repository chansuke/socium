//! Assembles a streamed Chat Completion into content blocks and the same
//! `ProviderEvent`s the Anthropic stream produces, so the turn loop and the
//! Activity pane do not depend on the wire.

use crate::agent::types::{AgentMessageResponse, ContentBlock};
use crate::providers::error::{ProviderError, describe_error};
use crate::providers::{BlockKind, ProviderEvent, ProviderResponse, TokenUsage};
use serde_json::Value;

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

struct ToolCallState {
    /// The call's `index` in the stream, which later fragments repeat.
    stream_index: u64,
    block: usize,
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
pub(super) struct ChatStreamBuilder {
    received_bytes: usize,
    next_block: usize,
    /// Reasoning text some servers stream (`reasoning_content` or
    /// `reasoning`). Shown in Activity only: there is no signature to send
    /// it back with, so it never enters the history.
    reasoning_block: Option<usize>,
    text: Option<(usize, String)>,
    tool_calls: Vec<ToolCallState>,
    finish_reason: Option<String>,
    usage: Option<TokenUsage>,
    done: bool,
    streamed_text: bool,
}

impl ChatStreamBuilder {
    pub(super) fn apply_chunk(
        &mut self,
        data: &str,
        emit: &mut impl FnMut(ProviderEvent),
    ) -> Result<(), ProviderError> {
        self.received_bytes = self.received_bytes.saturating_add(data.len());
        if self.received_bytes > MAX_RESPONSE_BYTES {
            return Err(ProviderError::Stream("Response exceeded 8 MiB".into()));
        }
        if data.trim() == "[DONE]" {
            self.done = true;
            return Ok(());
        }
        if self.done {
            return Err(ProviderError::Stream("Data after [DONE]".into()));
        }
        let chunk: Value = serde_json::from_str(data)?;
        if chunk.get("error").is_some_and(|error| !error.is_null()) {
            let detail =
                describe_error(&chunk["error"]).unwrap_or_else(|| "unknown error".to_string());
            return Err(ProviderError::Stream(format!("Streaming error: {detail}")));
        }
        if let Some(usage) = parse_usage(&chunk["usage"]) {
            self.usage = Some(usage);
        }
        // With include_usage the last chunk has no choices.
        let Some(choice) = chunk["choices"].as_array().and_then(|choices| {
            choices
                .iter()
                .find(|c| c["index"].as_u64().unwrap_or(0) == 0)
        }) else {
            return Ok(());
        };
        let delta = &choice["delta"];

        let reasoning = delta["reasoning_content"]
            .as_str()
            .or_else(|| delta["reasoning"].as_str());
        if let Some(text) = reasoning.filter(|text| !text.is_empty()) {
            let index = match self.reasoning_block {
                Some(index) => index,
                None => {
                    let index = self.start_block(BlockKind::Thinking, emit);
                    self.reasoning_block = Some(index);
                    index
                }
            };
            emit(ProviderEvent::ThinkingDelta {
                index,
                text: text.to_string(),
            });
        }

        if let Some(text) = delta["content"].as_str().filter(|text| !text.is_empty()) {
            self.close_reasoning(emit);
            let index = match &self.text {
                Some((index, _)) => *index,
                None => {
                    let index = self.start_block(BlockKind::Text, emit);
                    self.text = Some((index, String::new()));
                    index
                }
            };
            if let Some((_, buffer)) = self.text.as_mut() {
                buffer.push_str(text);
            }
            self.streamed_text = true;
            emit(ProviderEvent::TextDelta {
                index,
                text: text.to_string(),
            });
        }

        if let Some(calls) = delta["tool_calls"].as_array() {
            self.close_reasoning(emit);
            for call in calls {
                self.apply_tool_call(call, emit);
            }
        }

        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish_reason = Some(reason.to_string());
        }
        Ok(())
    }

    fn apply_tool_call(&mut self, call: &Value, emit: &mut impl FnMut(ProviderEvent)) {
        let stream_index = call["index"].as_u64();
        let position = match stream_index {
            Some(index) => self.tool_calls.iter().position(|t| t.stream_index == index),
            // Some servers send each call whole, without an index.
            None => call["id"]
                .as_str()
                .and_then(|id| self.tool_calls.iter().position(|t| t.id == id)),
        };
        let position = match position {
            Some(position) => position,
            None => {
                let block_index = self.next_block;
                let id = call["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map_or_else(|| format!("call_{block_index}"), str::to_string);
                let name = call["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let block = self.start_block(
                    BlockKind::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                    },
                    emit,
                );
                self.tool_calls.push(ToolCallState {
                    stream_index: stream_index.unwrap_or(self.tool_calls.len() as u64),
                    block,
                    id,
                    name,
                    arguments: String::new(),
                });
                self.tool_calls.len() - 1
            }
        };
        let state = &mut self.tool_calls[position];
        let fragment = match &call["function"]["arguments"] {
            Value::String(fragment) => fragment.clone(),
            Value::Null => String::new(),
            whole => whole.to_string(),
        };
        if !fragment.is_empty() {
            state.arguments.push_str(&fragment);
            emit(ProviderEvent::ToolInputDelta {
                index: state.block,
                partial_json: fragment,
            });
        }
    }

    fn start_block(&mut self, kind: BlockKind, emit: &mut impl FnMut(ProviderEvent)) -> usize {
        let index = self.next_block;
        self.next_block += 1;
        emit(ProviderEvent::BlockStarted { index, kind });
        index
    }

    fn close_reasoning(&mut self, emit: &mut impl FnMut(ProviderEvent)) {
        if let Some(index) = self.reasoning_block.take() {
            emit(ProviderEvent::BlockFinished { index });
        }
    }

    /// Closes the open blocks, reports the stop reason in Anthropic's terms,
    /// and checks that the response completed safely.
    pub(super) fn finish(
        mut self,
        emit: &mut impl FnMut(ProviderEvent),
    ) -> Result<ProviderResponse, ProviderError> {
        let Some(reason) = self.finish_reason.clone() else {
            return Err(ProviderError::Stream(
                "Response ended without a finish reason".into(),
            ));
        };
        self.close_reasoning(emit);
        let mut open = self
            .text
            .iter()
            .map(|(index, _)| *index)
            .chain(self.tool_calls.iter().map(|call| call.block))
            .collect::<Vec<_>>();
        open.sort_unstable();
        for index in open {
            emit(ProviderEvent::BlockFinished { index });
        }

        let has_tools = !self.tool_calls.is_empty();
        // Some compatible servers end a tool-calling turn with "stop".
        let stop_reason = match reason.as_str() {
            "tool_calls" | "stop" if has_tools => "tool_use",
            "stop" => "end_turn",
            "length" => "max_tokens",
            other => other,
        }
        .to_string();
        emit(ProviderEvent::MessageFinished {
            stop_reason: stop_reason.clone(),
            usage: self.usage,
        });
        match stop_reason.as_str() {
            "tool_use" | "end_turn" => {}
            "max_tokens" => {
                return Err(ProviderError::Stream(
                    "Response stopped at the output token limit (finish reason: length); \
                     raise max_output_tokens or MAX_TOKENS to allow longer responses"
                        .into(),
                ));
            }
            other => {
                return Err(ProviderError::Stream(format!(
                    "Response stopped without safe completion (finish reason: {other})"
                )));
            }
        }

        let mut blocks = Vec::with_capacity(1 + self.tool_calls.len());
        if let Some((index, text)) = self.text {
            blocks.push((index, ContentBlock::Text { text }));
        }
        for call in self.tool_calls {
            let input = if call.arguments.trim().is_empty() {
                Value::Object(Default::default())
            } else {
                serde_json::from_str(&call.arguments).map_err(|_| {
                    ProviderError::Stream(format!(
                        "Arguments of the {} tool call are not valid JSON",
                        call.name
                    ))
                })?
            };
            blocks.push((
                call.block,
                ContentBlock::ToolUse {
                    id: call.id,
                    name: call.name,
                    input,
                },
            ));
        }
        blocks.sort_by_key(|(index, _)| *index);
        Ok(ProviderResponse {
            message: AgentMessageResponse {
                content: blocks.into_iter().map(|(_, block)| block).collect(),
            },
            streamed_text: self.streamed_text,
            usage: self.usage,
        })
    }
}

/// Chat Completions usage: `prompt_tokens` includes cached tokens, which are
/// reported as cache reads.
fn parse_usage(usage: &Value) -> Option<TokenUsage> {
    let to_u32 = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
    let prompt = usage["prompt_tokens"].as_u64()?;
    let cached = usage["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0)
        .min(prompt);
    Some(TokenUsage {
        input_tokens: to_u32(prompt - cached),
        output_tokens: to_u32(usage["completion_tokens"].as_u64().unwrap_or(0)),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: to_u32(cached),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(chunks: &[Value]) -> (Result<ProviderResponse, ProviderError>, Vec<ProviderEvent>) {
        let mut events = Vec::new();
        let mut builder = ChatStreamBuilder::default();
        let mut emit = |event| events.push(event);
        for chunk in chunks {
            if let Err(err) = builder.apply_chunk(&chunk.to_string(), &mut emit) {
                return (Err(err), events);
            }
        }
        builder.apply_chunk("[DONE]", &mut emit).unwrap();
        let result = builder.finish(&mut emit);
        (result, events)
    }

    fn delta(delta: Value, finish: Option<&str>) -> Value {
        json!({"choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
    }

    fn usage_chunk() -> Value {
        json!({"choices": [], "usage": {
            "prompt_tokens": 120,
            "completion_tokens": 30,
            "prompt_tokens_details": {"cached_tokens": 100}
        }})
    }

    #[test]
    fn text_streams_and_ends_the_turn_with_usage() {
        let (result, events) = run(&[
            delta(json!({"role": "assistant", "content": ""}), None),
            delta(json!({"content": "Hel"}), None),
            delta(json!({"content": "lo"}), Some("stop")),
            usage_chunk(),
        ]);
        let response = result.unwrap();
        assert!(response.streamed_text);
        assert_eq!(
            response.message.content,
            [ContentBlock::Text {
                text: "Hello".into()
            }]
        );
        let usage = response.usage.unwrap();
        assert_eq!(
            (
                usage.input_tokens,
                usage.cache_read_input_tokens,
                usage.output_tokens
            ),
            (20, 100, 30)
        );
        assert_eq!(
            events,
            [
                ProviderEvent::BlockStarted {
                    index: 0,
                    kind: BlockKind::Text
                },
                ProviderEvent::TextDelta {
                    index: 0,
                    text: "Hel".into()
                },
                ProviderEvent::TextDelta {
                    index: 0,
                    text: "lo".into()
                },
                ProviderEvent::BlockFinished { index: 0 },
                ProviderEvent::MessageFinished {
                    stop_reason: "end_turn".into(),
                    usage: Some(usage),
                },
            ]
        );
    }

    #[test]
    fn tool_call_fragments_are_joined_per_index() {
        let (result, events) = run(&[
            delta(json!({"content": "Let me look."}), None),
            delta(
                json!({"tool_calls": [{"index": 0, "id": "call_a", "type": "function",
                    "function": {"name": "read_file", "arguments": "{\"pa"}}]}),
                None,
            ),
            delta(
                json!({"tool_calls": [{"index": 1, "id": "call_b", "type": "function",
                    "function": {"name": "git_status", "arguments": ""}}]}),
                None,
            ),
            delta(
                json!({"tool_calls": [{"index": 0, "function": {"arguments": "th\":\"a.rs\"}"}}]}),
                Some("tool_calls"),
            ),
        ]);
        let content = result.unwrap().message.content;
        assert_eq!(
            content,
            [
                ContentBlock::Text {
                    text: "Let me look.".into()
                },
                ContentBlock::ToolUse {
                    id: "call_a".into(),
                    name: "read_file".into(),
                    input: json!({"path": "a.rs"}),
                },
                ContentBlock::ToolUse {
                    id: "call_b".into(),
                    name: "git_status".into(),
                    input: json!({}),
                },
            ]
        );
        assert!(events.contains(&ProviderEvent::ToolInputDelta {
            index: 1,
            partial_json: "{\"pa".into()
        }));
        assert!(events.contains(&ProviderEvent::MessageFinished {
            stop_reason: "tool_use".into(),
            usage: None
        }));
    }

    #[test]
    fn whole_tool_calls_without_an_index_and_a_stop_reason_still_run() {
        let (result, _) = run(&[delta(
            json!({"tool_calls": [
                {"id": "c1", "function": {"name": "read_file", "arguments": {"path": "a"}}},
                {"id": "c2", "function": {"name": "read_file", "arguments": "{\"path\":\"b\"}"}}
            ]}),
            Some("stop"),
        )]);
        let content = result.unwrap().message.content;
        assert_eq!(content.len(), 2);
        assert!(matches!(&content[0], ContentBlock::ToolUse { input, .. } if input["path"] == "a"));
        assert!(matches!(&content[1], ContentBlock::ToolUse { input, .. } if input["path"] == "b"));
    }

    #[test]
    fn reasoning_is_shown_but_kept_out_of_the_history() {
        let (result, events) = run(&[
            delta(json!({"reasoning_content": "thinking"}), None),
            delta(json!({"content": "ok"}), Some("stop")),
        ]);
        assert_eq!(
            result.unwrap().message.content,
            [ContentBlock::Text { text: "ok".into() }]
        );
        assert_eq!(
            events[..3],
            [
                ProviderEvent::BlockStarted {
                    index: 0,
                    kind: BlockKind::Thinking
                },
                ProviderEvent::ThinkingDelta {
                    index: 0,
                    text: "thinking".into()
                },
                ProviderEvent::BlockFinished { index: 0 },
            ]
        );
    }

    #[test]
    fn unsafe_or_incomplete_endings_are_errors() {
        let (length, events) = run(&[delta(json!({"content": "cut"}), Some("length"))]);
        assert!(
            length
                .err()
                .unwrap()
                .to_string()
                .contains("output token limit")
        );
        assert!(events.contains(&ProviderEvent::MessageFinished {
            stop_reason: "max_tokens".into(),
            usage: None
        }));

        let (filtered, _) = run(&[delta(json!({}), Some("content_filter"))]);
        assert!(
            filtered
                .err()
                .unwrap()
                .to_string()
                .contains("finish reason: content_filter")
        );

        let (cut, _) = run(&[delta(json!({"content": "partial"}), None)]);
        assert!(
            cut.err()
                .unwrap()
                .to_string()
                .contains("without a finish reason")
        );

        let (bad_args, _) = run(&[delta(
            json!({"tool_calls": [{"index": 0, "id": "c", "function": {"name": "read_file", "arguments": "{"}}]}),
            Some("tool_calls"),
        )]);
        assert!(
            bad_args
                .err()
                .unwrap()
                .to_string()
                .contains("not valid JSON")
        );
    }

    #[test]
    fn stream_errors_and_data_after_done_are_rejected() {
        let mut builder = ChatStreamBuilder::default();
        let err = builder
            .apply_chunk(
                r#"{"error":{"message":"overloaded","code":"server_busy"}}"#,
                &mut |_| {},
            )
            .unwrap_err();
        assert_eq!(err.to_string(), "Streaming error: server_busy: overloaded");

        let mut builder = ChatStreamBuilder::default();
        builder.apply_chunk("[DONE]", &mut |_| {}).unwrap();
        assert!(builder.apply_chunk("{}", &mut |_| {}).is_err());
    }
}
