use crate::agent::context::{extend_summary_text, summarize_messages};
use crate::agent::turn::extract_text;
use crate::constants::{
    COMPACTION_MAX_OUTPUT_TOKENS, COMPACTION_RETAIN_RECENT_MESSAGES,
    COMPACTION_RETAIN_RECENT_TOKENS, COMPACTION_SOURCE_MESSAGE_MAX_CHARACTERS,
    COMPACTION_SYSTEM_PROMPT, COMPACTION_TRANSCRIPT_MAX_CHARACTERS, COMPACTION_TRIGGER_RATIO,
    COMPACTION_USER_MESSAGE_MAX_CHARACTERS, DEFAULT_COMPACT_AT_TOKENS,
};
use crate::conversation::{AgentMessageRequest, ContentBlock, Message, MessageContent, Role};
use crate::error::AgentError;
use crate::providers::error::ProviderError;
use crate::providers::{Provider, ProviderEvent, TokenUsage};
use crate::state::{ConversationSummary, ConversationSummaryState};

/// Heads a summary written without the compaction model.
const FALLBACK_SUMMARY_NOTE: &str = "(Line-by-line summary: the summarizer was unavailable, so decisions, open issues and verification results may be missing. Check before relying on it.)";

/// Prompt-token level at which auto-compaction fires: the model's
/// `compact_at_tokens` or the default, capped at a fixed ratio of the input
/// budget (the context window minus the output allowance) so compaction
/// runs before a request in the tool loop outgrows the budget. The caller
/// applies `COMPACTION_TOKEN_THRESHOLD` before this.
pub(crate) fn compaction_threshold_tokens(
    input_budget_tokens: u32,
    compact_at: Option<u32>,
) -> u32 {
    let ratio = (input_budget_tokens as f32 * COMPACTION_TRIGGER_RATIO) as u32;
    compact_at.unwrap_or(DEFAULT_COMPACT_AT_TOKENS).min(ratio)
}

/// Like [`retention_boundary`], but keeps at most `retain_messages` of the
/// newest messages and only as many as fit in about `retain_tokens`, so a
/// few huge tool results cannot make compaction a no-op. At least the last
/// message is kept.
pub(crate) fn retention_boundary_by_tokens(
    messages: &[Message],
    model: &str,
    retain_messages: usize,
    retain_tokens: u32,
) -> usize {
    let mut kept_tokens = 0u32;
    let mut keep = 0;
    for message in messages.iter().rev().take(retain_messages.max(1)) {
        let tokens = crate::agent::context::estimate_message_tokens(model, message);
        if keep > 0 && kept_tokens.saturating_add(tokens) > retain_tokens {
            break;
        }
        kept_tokens = kept_tokens.saturating_add(tokens);
        keep += 1;
    }
    retention_boundary(messages, keep)
}

/// Index separating messages to compact (before) from messages kept
/// verbatim (after). Starts `retain_recent` from the end, then moves
/// earlier while the boundary message is a tool_result-bearing user
/// message: a retained slice must never start with an orphaned
/// tool_result, or the API rejects the request.
pub(crate) fn retention_boundary(messages: &[Message], retain_recent: usize) -> usize {
    let mut boundary = messages.len().saturating_sub(retain_recent);
    while boundary > 0 && boundary < messages.len() && starts_with_tool_result(&messages[boundary])
    {
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

/// The summary state after a compaction, for its owner to save.
pub(crate) struct Compaction {
    pub(crate) summary_state: ConversationSummaryState,
    pub(crate) covered_messages: usize,
    /// Why the compaction model was not used, when it failed.
    pub(crate) fallback_reason: Option<String>,
    /// The compaction model and the usage it reported, when it answered.
    pub(crate) answered: Option<(String, Option<TokenUsage>)>,
}

impl Compaction {
    /// The activity event for this compaction, which the session's usage
    /// counts.
    pub(crate) fn finished_event(&self) -> crate::activity::ActivityKind {
        let (model, usage) = self.answered.clone().unwrap_or_default();
        crate::activity::ActivityKind::CompactionFinished {
            covered_messages: self.covered_messages,
            used_llm: self.fallback_reason.is_none(),
            model,
            usage,
        }
    }
}

/// Summarizes `messages` up to the newest ones kept verbatim, extending
/// the summary in `summary_state`. A failed compaction model leaves a
/// line-by-line summary instead. `NothingToCompact` when the summary
/// already covers everything before the kept messages.
pub(crate) async fn compact_history(
    summarizer: (&dyn Provider, &str),
    client: &reqwest::Client,
    control: &crate::TaskControl,
    session: Option<&str>,
    model: &str,
    messages: &[Message],
    summary_state: &ConversationSummaryState,
) -> Result<Compaction, AgentError> {
    let boundary = retention_boundary_by_tokens(
        messages,
        model,
        COMPACTION_RETAIN_RECENT_MESSAGES,
        COMPACTION_RETAIN_RECENT_TOKENS,
    );
    let previous = summary_state.summary.as_ref();
    let covered = previous.map_or(0, |summary| summary.covered_messages);
    if boundary <= covered {
        return Err(AgentError::NothingToCompact);
    }

    let previous_summary = previous.map(|summary| summary.text.as_str());
    let request = CompactionRequest {
        previous_summary,
        messages: &messages[covered..boundary],
        session,
    };
    let (provider, summary_model) = summarizer;
    let (summary_result, answered) = tokio::select! {
        biased;
        _ = control.cancelled() => return Err(AgentError::Cancelled),
        result = summarize_with_llm(provider, summary_model, client, &request) => result,
    };
    let answered = answered.map(|usage| (summary_model.to_string(), usage));
    let (text, fallback_reason) = match summary_result {
        Ok(text) => (text, None),
        Err(err) => {
            let text = match previous_summary {
                Some(previous) => {
                    let previous = previous
                        .strip_prefix(FALLBACK_SUMMARY_NOTE)
                        .map_or(previous, str::trim_start);
                    extend_summary_text(previous, messages, covered, boundary)
                }
                None => summarize_messages(&messages[..boundary]),
            };
            // The model reads this later; it must not take it for a full
            // summary.
            let text = format!("{FALLBACK_SUMMARY_NOTE}\n{text}");
            (
                text,
                Some(format!(
                    "The compaction model failed ({err}); kept a shorter line-by-line summary instead"
                )),
            )
        }
    };
    let first_request = previous
        .and_then(|summary| summary.first_request.clone())
        .or_else(|| ConversationSummary::first_request_of(&messages[..boundary]));
    Ok(Compaction {
        summary_state: ConversationSummaryState {
            summary: Some(ConversationSummary {
                covered_messages: boundary,
                text,
                // Everything after the cut so far was produced against the
                // replaced history.
                thinking_valid_from: Some(messages.len()),
                first_request,
            }),
        },
        covered_messages: boundary,
        fallback_reason,
        answered,
    })
}

pub(crate) struct CompactionRequest<'a> {
    /// Summary produced by an earlier compaction, so successive compactions
    /// extend coverage instead of re-reading the whole history.
    pub previous_summary: Option<&'a str>,
    /// Only the messages newly covered by this compaction.
    pub messages: &'a [Message],
    /// The conversation the summary is for; see
    /// [`AgentMessageRequest::session`].
    pub session: Option<&'a str>,
}

/// Summarize conversation history with a dedicated (cheap) model. Returns
/// an error on transport failure, an empty completion, or one cut at the
/// output limit; callers fall back to the deterministic summarizer. Also
/// returns, when the model answered, the usage it reported, since the call
/// is charged even when its summary is not used.
pub(crate) async fn summarize_with_llm(
    provider: &dyn Provider,
    model: &str,
    client: &reqwest::Client,
    request: &CompactionRequest<'_>,
) -> (Result<String, ProviderError>, Option<Option<TokenUsage>>) {
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

    let messages = [Message::user_text(prompt)];
    let message_request = AgentMessageRequest::with_system_prompt(
        model,
        COMPACTION_MAX_OUTPUT_TOKENS,
        COMPACTION_SYSTEM_PROMPT,
        &messages,
        &[],
    );
    let message_request = match request.session {
        Some(session) => message_request.in_session(session),
        None => message_request,
    };
    let mut discard_deltas = |_event: ProviderEvent| {};
    let response = match provider
        .send_message(client, &message_request, &mut discard_deltas)
        .await
    {
        Ok(response) => response,
        Err(err) => return (Err(err), None),
    };
    // Charged whether or not the summary below is used.
    let answered = Some(response.usage);

    // A summary cut mid-sentence would pass for a complete one and silently
    // lose its later sections; the deterministic summary says what it omits.
    if response.message.stop_reason.as_deref() == Some("max_tokens") {
        let err = ProviderError::Api(format!(
            "Compaction model's summary was cut at the output limit of {COMPACTION_MAX_OUTPUT_TOKENS} tokens"
        ));
        return (Err(err), answered);
    }
    let summary = extract_text(&response.message.content);
    let summary = summary.trim();
    if summary.is_empty() {
        let err = ProviderError::Api("Compaction model returned an empty summary".to_string());
        return (Err(err), answered);
    }
    (Ok(summary.to_string()), answered)
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
        let text_limit = match message.role {
            Role::User => COMPACTION_USER_MESSAGE_MAX_CHARACTERS,
            _ => COMPACTION_SOURCE_MESSAGE_MAX_CHARACTERS,
        };
        let detail = match &message.content {
            MessageContent::Text(text) => truncate_for_transcript(text, text_limit),
            MessageContent::Blocks(blocks) => render_blocks_for_transcript(blocks),
        };
        if !detail.is_empty() {
            lines.push(format!("{label}: {detail}"));
        }
    }
    crate::conversation::elide_tool_output(
        &lines.join("\n"),
        COMPACTION_TRANSCRIPT_MAX_CHARACTERS,
        "from the middle of the transcript",
    )
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
                let input_preview = truncate_for_transcript(&input.to_string(), 300);
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
            ContentBlock::Text { .. }
            | ContentBlock::Thinking { .. }
            | ContentBlock::RedactedThinking { .. } => {}
        }
    }

    parts.join(" ")
}

/// `text` on one line within `max_chars`, keeping its start and, with
/// twice the room, its end: a command's failure, a test's error or a
/// closing requirement usually comes last.
fn truncate_for_transcript(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let length = normalized.chars().count();
    if length <= max_chars {
        return normalized;
    }
    let marker = |omitted: usize| format!(" … {omitted} characters omitted … ");
    let room = max_chars.saturating_sub(marker(length).chars().count());
    let head = room / 3;
    let tail = room - head;
    let chars = normalized.chars().collect::<Vec<_>>();
    format!(
        "{}{}{}",
        chars[..head].iter().collect::<String>(),
        marker(length - head - tail),
        chars[length - tail..].iter().collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::{CompactionRequest, render_transcript_for_compaction, summarize_with_llm};
    use crate::constants::{COMPACTION_SYSTEM_PROMPT, DEFAULT_COMPACTION_MODEL};
    use crate::conversation::{ContentBlock, Message};
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
    fn retention_by_tokens_lets_compaction_cover_huge_recent_messages() {
        let big = || Message::user_text("z".repeat(100_000));
        let small = |i: usize| Message::user_text(format!("message {i}"));
        // Three huge messages at the end: only the last stays verbatim.
        let mut messages = (0..4).map(small).collect::<Vec<_>>();
        messages.extend([big(), big(), big()]);
        assert_eq!(
            super::retention_boundary_by_tokens(&messages, "claude", 16, 20_000),
            6
        );
        // Small messages keep the count limit.
        let messages = (0..30).map(small).collect::<Vec<_>>();
        assert_eq!(
            super::retention_boundary_by_tokens(&messages, "claude", 16, 20_000),
            14
        );
    }

    #[test]
    fn compaction_threshold_uses_the_lowest_of_model_default_and_ratio() {
        // A 1M window compacts at the default, not at 80% of the budget.
        assert_eq!(super::compaction_threshold_tokens(936_000, None), 200_000);
        assert_eq!(
            super::compaction_threshold_tokens(936_000, Some(400_000)),
            400_000
        );
        assert_eq!(super::compaction_threshold_tokens(136_000, None), 108_800);
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
    fn transcript_rendering_keeps_user_requests_and_truncates_the_rest() {
        // Review R5: a requirement at the end of a 1,400-character request
        // was cut at 600 characters and never reached the summary.
        let request = format!(
            "{} Also keep the public API unchanged.",
            "word ".repeat(280)
        );
        let messages = vec![
            Message::user_text(request),
            Message::assistant_blocks(vec![ContentBlock::Text {
                text: "word ".repeat(1_000),
            }]),
            Message::user_text("word ".repeat(3_000)),
        ];

        let rendered = render_transcript_for_compaction(&messages);
        let lines = rendered.lines().collect::<Vec<_>>();

        assert!(lines[0].ends_with("keep the public API unchanged."));
        assert!(lines[1].chars().count() < 700);
        assert!(lines[1].contains("characters omitted"), "{}", lines[1]);
        assert!(lines[2].chars().count() < 8_100);
        assert!(lines[2].contains("characters omitted"));
    }

    #[test]
    fn a_long_tool_result_keeps_the_error_at_its_end() {
        let output = format!(
            "{}\nerror[E0308]: mismatched types --> src/cli.rs:42:19",
            "   Compiling dep v0.1.0\n".repeat(150)
        );
        let messages = vec![Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "t1".into(),
            content: output,
            is_error: Some(true),
        }])];

        let rendered = render_transcript_for_compaction(&messages);

        assert!(
            rendered.contains("error[E0308]: mismatched types --> src/cli.rs:42:19"),
            "{rendered}"
        );
        assert!(rendered.contains("[tool_error Compiling dep"), "{rendered}");
        assert!(rendered.chars().count() < 700, "{rendered}");
    }

    #[tokio::test]
    async fn summarize_with_llm_sends_compaction_prompt_without_tools() {
        let provider = ScriptedProvider::new(vec![Ok(text_provider_response(
            "1. User request: fix the bug",
            None,
        ))]);
        let client = crate::test_support::http_client();
        let messages = transcript_messages();
        let request = CompactionRequest {
            previous_summary: Some("earlier summary text"),
            messages: &messages,
            session: None,
        };

        let (summary, answered) =
            summarize_with_llm(&provider, DEFAULT_COMPACTION_MODEL, &client, &request).await;
        assert_eq!(summary.unwrap(), "1. User request: fix the bug");
        assert!(answered.is_some(), "the call is counted");

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
        let client = crate::test_support::http_client();
        let messages = transcript_messages();
        let request = CompactionRequest {
            previous_summary: None,
            messages: &messages,
            session: None,
        };

        let (result, answered) =
            summarize_with_llm(&provider, DEFAULT_COMPACTION_MODEL, &client, &request).await;

        assert!(result.is_err());
        // Charged although its summary is not used.
        assert!(answered.is_some());
    }

    #[tokio::test]
    async fn a_summary_cut_at_the_output_limit_is_not_taken_as_complete() {
        let mut cut = text_provider_response("1. User request: fix the par", None);
        cut.message.stop_reason = Some("max_tokens".into());
        let provider = ScriptedProvider::new(vec![Ok(cut)]);
        let client = crate::test_support::http_client();
        let messages = transcript_messages();
        let request = CompactionRequest {
            previous_summary: None,
            messages: &messages,
            session: None,
        };

        let (result, answered) =
            summarize_with_llm(&provider, DEFAULT_COMPACTION_MODEL, &client, &request).await;
        let err = result.unwrap_err();

        assert!(err.to_string().contains("output limit"), "{err}");
        assert!(answered.is_some());
    }

    #[tokio::test]
    async fn a_fallback_summary_says_so_once_however_often_it_falls_back() {
        let failing = ScriptedProvider::new(vec![
            Err(crate::providers::error::ProviderError::Api("down".into())),
            Err(crate::providers::error::ProviderError::Api("down".into())),
        ]);
        let client = crate::test_support::http_client();
        let control = crate::TaskControl::default();
        let messages = (0..60)
            .map(|n| Message::user_text(format!("message {n} {}", "x".repeat(4_000))))
            .collect::<Vec<_>>();
        let first = super::compact_history(
            (&failing, DEFAULT_COMPACTION_MODEL),
            &client,
            &control,
            None,
            DEFAULT_COMPACTION_MODEL,
            &messages[..40],
            &super::ConversationSummaryState::default(),
        )
        .await
        .unwrap();
        assert!(first.fallback_reason.is_some());
        let second = super::compact_history(
            (&failing, DEFAULT_COMPACTION_MODEL),
            &client,
            &control,
            None,
            DEFAULT_COMPACTION_MODEL,
            &messages,
            &first.summary_state,
        )
        .await
        .unwrap();

        let text = &second.summary_state.summary.unwrap().text;
        assert!(text.starts_with(super::FALLBACK_SUMMARY_NOTE), "{text}");
        assert_eq!(
            text.matches(super::FALLBACK_SUMMARY_NOTE).count(),
            1,
            "{text}"
        );
    }
}
