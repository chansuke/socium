use crate::agent::turn::extract_text;
use crate::agent::types::{AgentMessageRequest, ContentBlock, Message, MessageContent, Role};
use crate::constants::{
    COMPACTION_MAX_OUTPUT_TOKENS, COMPACTION_SOURCE_MESSAGE_MAX_CHARACTERS,
    COMPACTION_SYSTEM_PROMPT, DEFAULT_COMPACTION_MODEL,
};
use crate::providers::error::ProviderError;
use crate::providers::{DeltaEvent, Provider};
use std::env;

/// Index separating messages to compact (before) from messages kept
/// verbatim (after). Starts `retain_recent` from the end, then moves
/// earlier while the boundary message is a tool_result-bearing user
/// message: a retained slice must never start with an orphaned
/// tool_result, or the API rejects the request.
pub(crate) fn retention_boundary(messages: &[Message], retain_recent: usize) -> usize {
    let mut boundary = messages.len().saturating_sub(retain_recent);
    while boundary > 0 && starts_with_tool_result(&messages[boundary]) {
        boundary -= 1;
    }
    boundary
}

fn starts_with_tool_result(message: &Message) -> bool {
    matches!(message.role, Role::User)
        && matches!(
            &message.content,
            MessageContent::Blocks(blocks)
                if blocks.iter().any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        )
}

pub(crate) struct CompactionRequest<'a> {
    /// Summary produced by an earlier compaction, so successive compactions
    /// extend coverage instead of re-reading the whole history.
    pub previous_summary: Option<&'a str>,
    /// Only the messages newly covered by this compaction.
    pub messages: &'a [Message],
}

pub(crate) fn compaction_model() -> String {
    env::var("COMPACT_MODEL").unwrap_or_else(|_| DEFAULT_COMPACTION_MODEL.to_string())
}

/// Summarize conversation history with a dedicated (cheap) model. Returns
/// an error on transport failure or an empty completion; callers fall back
/// to the deterministic summarizer.
pub(crate) async fn summarize_with_llm(
    provider: &dyn Provider,
    client: &reqwest::Client,
    request: &CompactionRequest<'_>,
) -> Result<String, ProviderError> {
    let transcript = render_transcript_for_compaction(request.messages);
    let mut prompt = String::new();
    if let Some(previous) = request.previous_summary {
        prompt.push_str("<previous_summary>\n");
        prompt.push_str(previous);
        prompt.push_str("\n</previous_summary>\n\n");
    }
    prompt.push_str("<transcript>\n");
    prompt.push_str(&transcript);
    prompt.push_str("\n</transcript>");

    let model = compaction_model();
    let messages = [Message::user_text(prompt)];
    let message_request = AgentMessageRequest::with_system_prompt(
        &model,
        COMPACTION_MAX_OUTPUT_TOKENS,
        COMPACTION_SYSTEM_PROMPT,
        &messages,
        &[],
    );
    let mut discard_deltas = |_event: DeltaEvent| {};
    let response = provider
        .send_message(client, &message_request, &mut discard_deltas)
        .await?;

    let summary = extract_text(&response.message.content);
    let summary = summary.trim();
    if summary.is_empty() {
        return Err(ProviderError::Api(
            "Compaction model returned an empty summary".to_string(),
        ));
    }
    Ok(summary.to_string())
}

/// Render messages as plain text for the compaction model. Long message
/// bodies are truncated; tool activity is kept visible because file paths
/// and tool errors are what later turns most often need.
fn render_transcript_for_compaction(messages: &[Message]) -> String {
    let mut lines = Vec::new();
    for message in messages {
        let label = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
        };
        let detail = match &message.content {
            MessageContent::Text(text) => {
                truncate_for_transcript(text, COMPACTION_SOURCE_MESSAGE_MAX_CHARACTERS)
            }
            MessageContent::Blocks(blocks) => render_blocks_for_transcript(blocks),
        };
        if !detail.is_empty() {
            lines.push(format!("{label}: {detail}"));
        }
    }
    lines.join("\n")
}

fn render_blocks_for_transcript(blocks: &[ContentBlock]) -> String {
    let mut parts = Vec::new();

    let text = extract_text(blocks);
    let truncated = truncate_for_transcript(&text, COMPACTION_SOURCE_MESSAGE_MAX_CHARACTERS);
    if !truncated.is_empty() {
        parts.push(truncated);
    }

    for block in blocks {
        match block {
            ContentBlock::ToolUse { name, input, .. } => {
                let input_preview = truncate_for_transcript(&input.to_string(), 120);
                parts.push(format!("[tool_use {name} {input_preview}]"));
            }
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                let marker = if is_error.unwrap_or(false) {
                    "tool_error"
                } else {
                    "tool_result"
                };
                let content_preview =
                    truncate_for_transcript(content, COMPACTION_SOURCE_MESSAGE_MAX_CHARACTERS);
                parts.push(format!("[{marker} {content_preview}]"));
            }
            ContentBlock::Text { .. } | ContentBlock::Thinking { .. } => {}
        }
    }

    parts.join(" ")
}

fn truncate_for_transcript(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut truncated = normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::{CompactionRequest, render_transcript_for_compaction, summarize_with_llm};
    use crate::agent::types::{ContentBlock, Message};
    use crate::constants::COMPACTION_SYSTEM_PROMPT;
    use crate::test_support::{ScriptedProvider, text_provider_response};
    use serde_json::json;

    fn transcript_messages() -> Vec<Message> {
        vec![
            Message::user_text("please fix the bug".to_string()),
            Message::assistant_blocks(vec![
                ContentBlock::Text {
                    text: "looking at src/lib.rs".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "read_file".to_string(),
                    input: json!({ "path": "src/lib.rs" }),
                },
            ]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "file not found".to_string(),
                is_error: Some(true),
            }]),
        ]
    }

    #[test]
    fn retention_boundary_keeps_requested_recent_count() {
        let messages: Vec<Message> = (0..20)
            .map(|index| Message::user_text(format!("message {index}")))
            .collect();

        assert_eq!(super::retention_boundary(&messages, 16), 4);
        assert_eq!(super::retention_boundary(&messages, 30), 0);
    }

    #[test]
    fn retention_boundary_never_starts_retained_slice_with_tool_result() {
        let mut messages: Vec<Message> = (0..4)
            .map(|index| Message::user_text(format!("message {index}")))
            .collect();
        messages.push(Message::assistant_blocks(vec![ContentBlock::ToolUse {
            id: "tool_1".to_string(),
            name: "read_file".to_string(),
            input: json!({}),
        }]));
        messages.push(Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "tool_1".to_string(),
            content: "ok".to_string(),
            is_error: None,
        }]));
        messages.push(Message::user_text("after".to_string()));

        // Naive boundary (len 7 - retain 2 = 5) would start with the
        // tool_result message; it must move back past the tool_use pair.
        assert_eq!(super::retention_boundary(&messages, 2), 4);
    }

    #[test]
    fn transcript_rendering_keeps_tool_names_and_errors() {
        let rendered = render_transcript_for_compaction(&transcript_messages());

        assert!(rendered.contains("User: please fix the bug"));
        assert!(rendered.contains("read_file"));
        assert!(rendered.contains("tool_error"));
        assert!(rendered.contains("file not found"));
    }

    #[test]
    fn transcript_rendering_truncates_long_messages() {
        let messages = vec![Message::user_text("word ".repeat(1_000))];

        let rendered = render_transcript_for_compaction(&messages);

        assert!(rendered.chars().count() < 700);
        assert!(rendered.ends_with('…'));
    }

    #[tokio::test]
    async fn summarize_with_llm_sends_compaction_prompt_without_tools() {
        let provider = ScriptedProvider::new(vec![Ok(text_provider_response(
            "1. User request: fix the bug",
            None,
        ))]);
        let client = reqwest::Client::builder().build().unwrap();
        let messages = transcript_messages();
        let request = CompactionRequest {
            previous_summary: Some("earlier summary text"),
            messages: &messages,
        };

        let summary = summarize_with_llm(&provider, &client, &request)
            .await
            .unwrap();
        assert_eq!(summary, "1. User request: fix the bug");

        let recorded = provider.recorded_requests.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["system"], COMPACTION_SYSTEM_PROMPT);
        assert!(recorded[0]["tools"].as_array().unwrap().is_empty());
        let prompt = recorded[0]["messages"][0]["content"].as_str().unwrap();
        assert!(prompt.contains("<previous_summary>\nearlier summary text"));
        assert!(prompt.contains("please fix the bug"));
    }

    #[tokio::test]
    async fn summarize_with_llm_rejects_empty_summary() {
        let provider = ScriptedProvider::new(vec![Ok(text_provider_response("   ", None))]);
        let client = reqwest::Client::builder().build().unwrap();
        let messages = transcript_messages();
        let request = CompactionRequest {
            previous_summary: None,
            messages: &messages,
        };

        let result = summarize_with_llm(&provider, &client, &request).await;

        assert!(result.is_err());
    }
}
