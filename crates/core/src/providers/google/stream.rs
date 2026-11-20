//! Assembles a streamed `streamGenerateContent` reply into content blocks
//! and the same `ProviderEvent`s the other wires produce, so the turn loop
//! and the Activity pane do not depend on the wire.
//!
//! Each SSE event is a whole `GenerateContentResponse` holding the next
//! parts of the first candidate. A thought signature rides on the part it
//! belongs to; it is kept as a thinking block placed just before the block
//! of that part, so the request side can put it back in the same place.

use super::{encode_signature, generated_call_id};
use crate::agent::types::{AgentMessageResponse, ContentBlock};
use crate::providers::error::{ProviderError, describe_error};
use crate::providers::{BlockKind, ProviderEvent, ProviderResponse, TokenUsage};
use serde_json::Value;

const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

enum PartKind {
    Text(String),
    Thought(String),
    Call {
        id: String,
        name: String,
        args: Value,
    },
}

struct BlockState {
    block: usize,
    kind: PartKind,
    signature: Option<String>,
    finished: bool,
}

#[derive(Default)]
pub(super) struct GoogleStreamBuilder {
    received_bytes: usize,
    blocks: Vec<BlockState>,
    finish_reason: Option<String>,
    usage: Option<TokenUsage>,
    streamed_text: bool,
}

impl GoogleStreamBuilder {
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
        if event["error"].is_object() {
            return Err(stream_error(&event["error"]));
        }
        // Each event repeats the running totals; the last one is final.
        if let Some(usage) = parse_usage(&event["usageMetadata"]) {
            self.usage = Some(usage);
        }
        if let Some(reason) = event["promptFeedback"]["blockReason"].as_str() {
            // A blocked prompt gets no candidates at all.
            self.finish_reason = Some(format!("PROMPT_BLOCKED_{reason}"));
            return Ok(());
        }
        let candidate = &event["candidates"][0];
        let parts = candidate["content"]["parts"].as_array();
        if self.finish_reason.is_some() && parts.is_some_and(|parts| !parts.is_empty()) {
            return Err(ProviderError::Stream(
                "Response sent more content after it finished".into(),
            ));
        }
        for part in parts.into_iter().flatten() {
            self.apply_part(part, emit);
        }
        if let Some(reason) = candidate["finishReason"].as_str() {
            self.finish_reason = Some(reason.to_string());
        }
        Ok(())
    }

    fn apply_part(&mut self, part: &Value, emit: &mut impl FnMut(ProviderEvent)) {
        let signature = part["thoughtSignature"]
            .as_str()
            .filter(|signature| !signature.is_empty())
            .map(str::to_string);
        if let Some(call) = part.get("functionCall") {
            let id = call["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map_or_else(generated_call_id, str::to_string);
            let name = call["name"].as_str().unwrap_or_default().to_string();
            let args = call
                .get("args")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()));
            let block = self.start(
                PartKind::Call {
                    id: id.clone(),
                    name: name.clone(),
                    args: args.clone(),
                },
                BlockKind::ToolUse { id, name },
                signature,
                emit,
            );
            // The call arrives whole, so its input is one delta.
            emit(ProviderEvent::ToolInputDelta {
                index: block,
                partial_json: args.to_string(),
            });
            self.finish_last(emit);
            return;
        }
        let Some(text) = part["text"].as_str() else {
            return;
        };
        let thought = part["thought"].as_bool() == Some(true);
        if text.is_empty() && signature.is_none() {
            return;
        }
        // Text joins the open block of its kind, so streamed chunks read as
        // one answer. Two signatures never share a block, and an empty part
        // that only carries a signature stays its own part, as sent.
        let joins = !text.is_empty()
            && self.blocks.last().is_some_and(|last| {
                !last.finished
                    && (last.signature.is_none() || signature.is_none())
                    && matches!(
                        (&last.kind, thought),
                        (PartKind::Thought(_), true) | (PartKind::Text(_), false)
                    )
            });
        if !joins {
            self.finish_last(emit);
            let (kind, block_kind) = if thought {
                (PartKind::Thought(String::new()), BlockKind::Thinking)
            } else {
                (PartKind::Text(String::new()), BlockKind::Text)
            };
            self.start(kind, block_kind, None, emit);
        }
        let last = self.blocks.last_mut().expect("a block was just opened");
        if signature.is_some() {
            last.signature = signature;
        }
        if text.is_empty() {
            return;
        }
        let index = last.block;
        match &mut last.kind {
            PartKind::Thought(buffer) => {
                buffer.push_str(text);
                emit(ProviderEvent::ThinkingDelta {
                    index,
                    text: text.to_string(),
                });
            }
            PartKind::Text(buffer) => {
                buffer.push_str(text);
                self.streamed_text = true;
                emit(ProviderEvent::TextDelta {
                    index,
                    text: text.to_string(),
                });
            }
            PartKind::Call { .. } => {}
        }
    }

    fn start(
        &mut self,
        kind: PartKind,
        block_kind: BlockKind,
        signature: Option<String>,
        emit: &mut impl FnMut(ProviderEvent),
    ) -> usize {
        self.finish_last(emit);
        let block = self.blocks.len();
        emit(ProviderEvent::BlockStarted {
            index: block,
            kind: block_kind,
        });
        self.blocks.push(BlockState {
            block,
            kind,
            signature,
            finished: false,
        });
        block
    }

    fn finish_last(&mut self, emit: &mut impl FnMut(ProviderEvent)) {
        if let Some(last) = self.blocks.last_mut()
            && !last.finished
        {
            last.finished = true;
            emit(ProviderEvent::BlockFinished { index: last.block });
        }
    }

    /// Closes the open block, reports the stop reason in Anthropic's terms,
    /// and checks that the response completed safely.
    pub(super) fn finish(
        mut self,
        emit: &mut impl FnMut(ProviderEvent),
    ) -> Result<ProviderResponse, ProviderError> {
        self.finish_last(emit);
        let has_tools = self
            .blocks
            .iter()
            .any(|state| matches!(state.kind, PartKind::Call { .. }));
        let stop_reason = self
            .finish_reason
            .as_deref()
            .map(|reason| stop_reason(reason, has_tools));
        if let Some(stop_reason) = &stop_reason {
            emit(ProviderEvent::MessageFinished {
                stop_reason: stop_reason.clone(),
                usage: self.usage,
            });
        }
        crate::providers::stop_reason_allows_release(stop_reason.as_deref(), has_tools)?;

        let mut blocks = Vec::with_capacity(self.blocks.len());
        let mut invalid_tool_inputs = Vec::new();
        // Thought text has no signature of its own on Gemini 3 and later; it
        // is kept with the next signature, just before the part that
        // carries it. Thought text after the last signature cannot be sent
        // back and stays out of the history.
        let mut thought = String::new();
        for state in self.blocks {
            if let PartKind::Thought(text) = &state.kind {
                if !thought.is_empty() && !text.is_empty() {
                    thought.push_str("\n\n");
                }
                thought.push_str(text);
            }
            if let Some(signature) = &state.signature {
                blocks.push(ContentBlock::Thinking {
                    thinking: std::mem::take(&mut thought),
                    signature: encode_signature(signature),
                });
            }
            match state.kind {
                PartKind::Thought(_) => {}
                PartKind::Text(text) => {
                    if !text.is_empty() {
                        blocks.push(ContentBlock::Text { text });
                    }
                }
                PartKind::Call { id, name, args } => {
                    let input = if args.is_object() {
                        args
                    } else {
                        invalid_tool_inputs
                            .push((id.clone(), "the input is not a JSON object".to_string()));
                        Value::Object(Default::default())
                    };
                    blocks.push(ContentBlock::ToolUse { id, name, input });
                }
            }
        }
        Ok(ProviderResponse {
            message: AgentMessageResponse {
                content: blocks,
                stop_reason: stop_reason.filter(|reason| reason == "max_tokens"),
                invalid_tool_inputs,
            },
            streamed_text: self.streamed_text,
            usage: self.usage,
        })
    }
}

/// `finishReason` in Anthropic's terms. Reasons with no counterpart pass
/// through, so the release check rejects them by name.
fn stop_reason(reason: &str, has_tools: bool) -> String {
    match reason {
        "STOP" if has_tools => "tool_use",
        "STOP" => "end_turn",
        "MAX_TOKENS" => "max_tokens",
        "SAFETY" | "RECITATION" | "PROHIBITED_CONTENT" | "BLOCKLIST" | "SPII" => "refusal",
        blocked if blocked.starts_with("PROMPT_BLOCKED_") => "refusal",
        other => other,
    }
    .to_string()
}

/// An error object sent in the stream. Overload, rate limits, and internal
/// errors are worth another attempt; invalid requests are not.
fn stream_error(error: &Value) -> ProviderError {
    let detail = describe_error(error).unwrap_or_else(|| "unknown error".to_string());
    let message = format!("Streaming error: {detail}");
    let retryable = matches!(error["code"].as_u64(), Some(429 | 500 | 503 | 504))
        || matches!(
            error["status"].as_str(),
            Some("UNAVAILABLE" | "INTERNAL" | "DEADLINE_EXCEEDED")
        );
    if retryable {
        ProviderError::Transient(message)
    } else {
        ProviderError::Stream(message)
    }
}

/// `usageMetadata`: `promptTokenCount` includes the cached tokens, which
/// are reported as cache reads; thought tokens are billed as output.
fn parse_usage(usage: &Value) -> Option<TokenUsage> {
    let to_u32 = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
    let prompt = usage["promptTokenCount"].as_u64()?;
    let cached = usage["cachedContentTokenCount"]
        .as_u64()
        .unwrap_or(0)
        .min(prompt);
    let output = usage["candidatesTokenCount"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(usage["thoughtsTokenCount"].as_u64().unwrap_or(0));
    Some(TokenUsage {
        input_tokens: to_u32(prompt - cached),
        output_tokens: to_u32(output),
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: to_u32(cached),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(events: &[Value]) -> (Result<ProviderResponse, ProviderError>, Vec<ProviderEvent>) {
        let mut emitted = Vec::new();
        let mut builder = GoogleStreamBuilder::default();
        let mut emit = |event| emitted.push(event);
        for event in events {
            if let Err(err) = builder.apply_event(&event.to_string(), &mut emit) {
                return (Err(err), emitted);
            }
        }
        let result = builder.finish(&mut emit);
        (result, emitted)
    }

    fn parts(parts: Value) -> Value {
        json!({"candidates": [{"content": {"role": "model", "parts": parts}}]})
    }

    fn finished(reason: &str) -> Value {
        json!({"candidates": [{"content": {"role": "model", "parts": []},
            "finishReason": reason}]})
    }

    fn stop_of(events: &[ProviderEvent]) -> Option<String> {
        events.iter().find_map(|event| match event {
            ProviderEvent::MessageFinished { stop_reason, .. } => Some(stop_reason.clone()),
            _ => None,
        })
    }

    #[test]
    fn thoughts_text_and_a_call_become_blocks_with_the_signature_kept() {
        let (result, events) = run(&[
            parts(json!([{"text": "Look at ", "thought": true}])),
            parts(json!([{"text": "the file.", "thought": true}])),
            parts(json!([{"text": "Reading "}])),
            parts(json!([{"text": "it."}])),
            json!({"candidates": [{"content": {"role": "model", "parts": [
                {"functionCall": {"id": "fc_1", "name": "read_file",
                    "args": {"path": "a.rs"}}, "thoughtSignature": "sig-A"},
                {"functionCall": {"id": "fc_2", "name": "read_file",
                    "args": {"path": "b.rs"}}}
            ]}, "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 120, "cachedContentTokenCount": 100,
                    "candidatesTokenCount": 20, "thoughtsTokenCount": 10,
                    "totalTokenCount": 150}}),
        ]);
        let response = result.unwrap();
        assert_eq!(
            response.message.content,
            [
                ContentBlock::Text {
                    text: "Reading it.".into()
                },
                ContentBlock::Thinking {
                    thinking: "Look at the file.".into(),
                    signature: encode_signature("sig-A"),
                },
                ContentBlock::ToolUse {
                    id: "fc_1".into(),
                    name: "read_file".into(),
                    input: json!({"path": "a.rs"}),
                },
                ContentBlock::ToolUse {
                    id: "fc_2".into(),
                    name: "read_file".into(),
                    input: json!({"path": "b.rs"}),
                },
            ]
        );
        assert!(response.streamed_text);
        assert_eq!(
            response.usage,
            Some(TokenUsage {
                input_tokens: 20,
                output_tokens: 30,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 100,
            })
        );
        assert_eq!(
            events[..4],
            [
                ProviderEvent::BlockStarted {
                    index: 0,
                    kind: BlockKind::Thinking
                },
                ProviderEvent::ThinkingDelta {
                    index: 0,
                    text: "Look at ".into()
                },
                ProviderEvent::ThinkingDelta {
                    index: 0,
                    text: "the file.".into()
                },
                ProviderEvent::BlockFinished { index: 0 },
            ]
        );
        assert!(events.contains(&ProviderEvent::BlockStarted {
            index: 2,
            kind: BlockKind::ToolUse {
                id: "fc_1".into(),
                name: "read_file".into()
            }
        }));
        assert!(events.contains(&ProviderEvent::ToolInputDelta {
            index: 2,
            partial_json: r#"{"path":"a.rs"}"#.into()
        }));
        assert_eq!(stop_of(&events).as_deref(), Some("tool_use"));
    }

    #[test]
    fn thinking_right_before_a_signed_part_is_kept_with_its_signature() {
        let (result, _) = run(&[
            parts(json!([{"text": "Plan.", "thought": true}])),
            parts(json!([{"functionCall": {"name": "list_files", "args": {}},
                "thoughtSignature": "sig-B"}])),
            finished("STOP"),
        ]);
        let content = result.unwrap().message.content;
        assert_eq!(
            content[0],
            ContentBlock::Thinking {
                thinking: "Plan.".into(),
                signature: encode_signature("sig-B"),
            }
        );
        // The API sent no id, so one is made up.
        let ContentBlock::ToolUse { id, name, .. } = &content[1] else {
            panic!("{content:?}");
        };
        assert_eq!(name, "list_files");
        assert!(super::super::is_generated_call_id(id), "{id}");
    }

    #[test]
    fn a_signature_in_a_trailing_empty_text_part_is_kept() {
        let (result, events) = run(&[
            parts(json!([{"text": "Done."}])),
            parts(json!([{"text": "", "thoughtSignature": "sig-C"}])),
            finished("STOP"),
        ]);
        assert_eq!(
            result.unwrap().message.content,
            [
                ContentBlock::Text {
                    text: "Done.".into()
                },
                ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: encode_signature("sig-C"),
                },
            ]
        );
        assert_eq!(stop_of(&events).as_deref(), Some("end_turn"));
    }

    #[test]
    fn finish_reasons_map_to_stop_reasons() {
        for (reason, expected) in [
            ("MAX_TOKENS", "max_tokens"),
            ("SAFETY", "refusal"),
            ("RECITATION", "refusal"),
            ("PROHIBITED_CONTENT", "refusal"),
            ("BLOCKLIST", "refusal"),
            ("SPII", "refusal"),
            ("MALFORMED_FUNCTION_CALL", "MALFORMED_FUNCTION_CALL"),
        ] {
            let (result, events) = run(&[parts(json!([{"text": "Hi"}])), finished(reason)]);
            assert_eq!(stop_of(&events).as_deref(), Some(expected), "{reason}");
            if expected == "max_tokens" {
                let response = result.unwrap();
                assert_eq!(response.message.stop_reason.as_deref(), Some("max_tokens"));
            } else {
                let err = result.err().unwrap().to_string();
                assert!(err.contains(expected), "{reason}: {err}");
            }
        }
        let (result, _) = run(&[
            parts(json!([{"functionCall": {"name": "read_file", "args": {"path": "a"}}}])),
            finished("MAX_TOKENS"),
        ]);
        assert!(result.err().unwrap().to_string().contains("tool call"));
    }

    #[test]
    fn a_blocked_prompt_is_a_refusal_and_a_cut_stream_is_an_error() {
        let (result, events) = run(&[json!({"promptFeedback": {"blockReason": "SAFETY"}})]);
        assert_eq!(stop_of(&events).as_deref(), Some("refusal"));
        assert!(result.is_err());

        let (result, _) = run(&[parts(json!([{"text": "Hal"}]))]);
        assert_eq!(
            result.err().unwrap().to_string(),
            "Response ended without a stop reason"
        );
    }

    #[test]
    fn errors_in_the_stream_are_retried_only_when_transient() {
        let mut builder = GoogleStreamBuilder::default();
        let err = builder
            .apply_event(
                r#"{"error":{"code":503,"message":"The model is overloaded.","status":"UNAVAILABLE"}}"#,
                &mut |_| {},
            )
            .unwrap_err();
        assert!(err.is_retryable());
        assert_eq!(
            err.to_string(),
            "Streaming error: UNAVAILABLE: The model is overloaded."
        );
        let err = builder
            .apply_event(
                r#"{"error":{"code":400,"message":"Bad schema","status":"INVALID_ARGUMENT"}}"#,
                &mut |_| {},
            )
            .unwrap_err();
        assert!(!err.is_retryable());
    }

    #[test]
    fn content_after_the_finish_is_rejected() {
        let (result, _) = run(&[finished("STOP"), parts(json!([{"text": "more"}]))]);
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("after it finished")
        );
    }
}
