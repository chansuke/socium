use crate::agent::state::{ConversationSummary, ConversationSummaryState};
use crate::agent::turn::extract_text;
use crate::constants::{
    DEFAULT_CONTEXT_WINDOW_TOKENS, MAX_SUMMARY_CHARACTERS, MAX_SUMMARY_LINE_CHARACTERS,
    MAX_SUMMARY_LINES, MIN_SHRUNK_TOOL_RESULT_CHARACTERS, TARGET_RECENT_CONTEXT_MESSAGES,
};
use crate::conversation::ToolDefinition;
use crate::conversation::{
    AgentMessageRequest, ContentBlock, Message, MessageContent, Role, strip_thinking,
};
use std::io::{self, Error, ErrorKind};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ContextBudget {
    pub context_window_tokens: u32,
    pub max_output_tokens: u32,
    /// Measured input tokens per estimated token on the last request, so
    /// text the character heuristic undercounts (such as Japanese) still
    /// fits. 1.0 until a provider reports usage.
    pub estimate_scale: f32,
}

impl ContextBudget {
    pub(crate) fn input_budget_tokens(self) -> u32 {
        self.context_window_tokens
            .saturating_sub(self.max_output_tokens)
            .max(1)
    }

    pub(crate) fn with_scale(mut self, estimate_scale: f32) -> Self {
        self.estimate_scale = estimate_scale;
        self
    }

    /// The estimate for a request, corrected by the last measurement.
    pub(crate) fn estimate(
        self,
        model: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> u32 {
        let raw = estimate_request_tokens(model, self.max_output_tokens, messages, tools);
        (raw as f32 * self.estimate_scale).ceil() as u32
    }
}

/// The measured-to-estimated ratio, bounded so one odd measurement (a
/// cached prompt, a provider that counts differently) cannot swing it far.
pub(crate) fn estimate_scale(measured: u32, estimated: u32) -> Option<f32> {
    (measured > 0 && estimated > 0).then(|| (measured as f32 / estimated as f32).clamp(0.5, 3.0))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextMetrics {
    pub estimated_input_tokens: u32,
    pub input_budget_tokens: u32,
    pub context_window_tokens: u32,
    pub max_output_tokens: u32,
    pub context_left_percentage: u32,
    pub summary_active: bool,
    pub summarized_messages: usize,
    /// Input tokens measured by the provider on the most recent call, when
    /// available. Preferred over `estimated_input_tokens` for display and
    /// compaction thresholds.
    pub measured_input_tokens: Option<u32>,
}

impl ContextMetrics {
    /// Overlay a provider-measured input size onto heuristic metrics,
    /// recomputing the remaining-context percentage from the measurement.
    pub(crate) fn apply_measured_input(&mut self, measured_input_tokens: Option<u32>) {
        self.measured_input_tokens = measured_input_tokens;
        if let Some(measured) = measured_input_tokens {
            self.context_left_percentage = context_left_percentage(
                self.context_window_tokens,
                measured,
                self.max_output_tokens,
            );
        }
    }
}

pub(crate) struct PreparedContext {
    pub messages: Vec<Message>,
    pub summary_state: ConversationSummaryState,
    pub metrics: ContextMetrics,
}

/// The budget for a model with this window (`CONTEXT_WINDOW_TOKENS` or the
/// model's configured one, read by the caller), which defaults to
/// `DEFAULT_CONTEXT_WINDOW_TOKENS`.
pub(crate) fn resolve_context_budget(
    configured_window: Option<u32>,
    max_output_tokens: u32,
) -> ContextBudget {
    ContextBudget {
        context_window_tokens: configured_window.unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS),
        max_output_tokens,
        estimate_scale: 1.0,
    }
}

pub(crate) fn prepare_context_messages(
    raw_messages: &[Message],
    summary_state: &ConversationSummaryState,
    budget: ContextBudget,
    model: &str,
    tools: &[ToolDefinition],
) -> io::Result<PreparedContext> {
    let mut prepared =
        prepare_context_with_budget(raw_messages, summary_state, model, budget, tools);
    if prepared.metrics.estimated_input_tokens > budget.input_budget_tokens() {
        prepared = shrink_to_budget(prepared, model, budget, tools);
    }
    if prepared.metrics.estimated_input_tokens > budget.input_budget_tokens() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!(
                "The latest message alone is about {} tokens, over the input budget of {}; send a shorter message",
                prepared.metrics.estimated_input_tokens,
                budget.input_budget_tokens()
            ),
        ));
    }
    Ok(prepared)
}

pub(crate) fn context_metrics_for_input(
    raw_messages: &[Message],
    pending_user_input: &str,
    summary_state: &ConversationSummaryState,
    budget: ContextBudget,
    model: &str,
    tools: &[ToolDefinition],
) -> io::Result<ContextMetrics> {
    let mut candidate_messages = raw_messages.to_vec();
    let trimmed = pending_user_input.trim();
    if !trimmed.is_empty() {
        candidate_messages.push(Message::user_text(trimmed.to_string()));
    }
    Ok(
        prepare_context_with_budget(&candidate_messages, summary_state, model, budget, tools)
            .metrics,
    )
}

/// Estimated prompt size of the whole history, after an earlier compaction
/// but before any budget trimming, so a smaller window can be noticed.
pub(crate) fn history_input_tokens(
    raw_messages: &[Message],
    summary_state: &ConversationSummaryState,
    model: &str,
    budget: ContextBudget,
    tools: &[ToolDefinition],
) -> u32 {
    if raw_messages.is_empty() {
        return 0;
    }
    let compacted_covered = summary_state
        .summary
        .as_ref()
        .map(|summary| summary.covered_messages)
        .unwrap_or(0)
        .min(raw_messages.len());
    compose_context(
        raw_messages,
        summary_state,
        compacted_covered,
        model,
        budget,
        tools,
    )
    .metrics
    .estimated_input_tokens
}

fn prepare_context_with_budget(
    raw_messages: &[Message],
    summary_state: &ConversationSummaryState,
    model: &str,
    budget: ContextBudget,
    tools: &[ToolDefinition],
) -> PreparedContext {
    if raw_messages.is_empty() {
        return PreparedContext {
            messages: Vec::new(),
            summary_state: ConversationSummaryState::default(),
            metrics: build_metrics(budget, 0, false, 0),
        };
    }

    // A persisted compaction fixes the minimum coverage: never resurrect
    // messages an earlier compaction already summarized.
    let compacted_covered = summary_state
        .summary
        .as_ref()
        .map(|summary| summary.covered_messages)
        .unwrap_or(0)
        .min(raw_messages.len());

    let whole = compose_context(
        raw_messages,
        summary_state,
        compacted_covered,
        model,
        budget,
        tools,
    );
    if whole.metrics.estimated_input_tokens <= budget.input_budget_tokens() {
        return whole;
    }
    let base_keep_recent = raw_messages.len().min(TARGET_RECENT_CONTEXT_MESSAGES);
    let mut best = compose_context(
        raw_messages,
        summary_state,
        raw_messages
            .len()
            .saturating_sub(base_keep_recent)
            .max(compacted_covered),
        model,
        budget,
        tools,
    );

    if best.metrics.estimated_input_tokens <= best.metrics.input_budget_tokens {
        return best;
    }

    // Keep fewer messages, down to the latest one, before cutting into
    // what is kept.
    for keep_recent in (1..base_keep_recent).rev() {
        let candidate = compose_context(
            raw_messages,
            summary_state,
            raw_messages
                .len()
                .saturating_sub(keep_recent)
                .max(compacted_covered),
            model,
            budget,
            tools,
        );
        if candidate.metrics.estimated_input_tokens <= candidate.metrics.input_budget_tokens {
            return candidate;
        }
        best = candidate;
    }

    best
}

/// When even the fewest kept messages are too large, a kept message holds
/// huge tool output: the oldest results are shrunk first. Thinking after a
/// shrunk result was produced against the full text, so it is not sent.
/// Only a request does this; the metrics shown as the user types do not.
fn shrink_to_budget(
    mut best: PreparedContext,
    model: &str,
    budget: ContextBudget,
    tools: &[ToolDefinition],
) -> PreparedContext {
    let limit = budget.input_budget_tokens();
    let mut estimate = best.metrics.estimated_input_tokens;
    for index in 0..best.messages.len() {
        if estimate <= limit {
            break;
        }
        let MessageContent::Blocks(blocks) = &mut best.messages[index].content else {
            continue;
        };
        let excess = excess_characters(model, estimate - limit, budget.estimate_scale);
        if shrink_tool_results(blocks, excess) == 0 {
            continue;
        }
        strip_thinking(&mut best.messages[index + 1..]);
        estimate = budget.estimate(model, &best.messages, tools);
    }
    best.metrics = build_metrics(
        budget,
        estimate,
        best.metrics.summary_active,
        best.metrics.summarized_messages,
    );
    best
}

/// Roughly how many characters hold `tokens` tokens of `model`'s text.
pub(crate) fn excess_characters(model: &str, tokens: u32, estimate_scale: f32) -> usize {
    // A small margin: the cut adds a marker, and the estimate rounds.
    ((tokens as f32 * chars_per_token_for_model(model) / estimate_scale) * 1.05) as usize + 256
}

/// Cuts the largest tool results in `blocks` to their head and tail until
/// about `excess` characters are gone, never below
/// `MIN_SHRUNK_TOOL_RESULT_CHARACTERS` each. Returns how many characters
/// were removed.
pub(crate) fn shrink_tool_results(blocks: &mut [ContentBlock], excess: usize) -> usize {
    // A cut adds a marker line; cutting less than this would not shorten.
    const MARKER: usize = 160;
    let mut removed = 0;
    while removed < excess {
        let largest = blocks
            .iter_mut()
            .filter_map(|block| match block {
                ContentBlock::ToolResult { content, .. } => Some(content),
                _ => None,
            })
            .max_by_key(|content| content.chars().count());
        let Some(content) = largest else {
            break;
        };
        let length = content.chars().count();
        let keep = length
            .saturating_sub((excess - removed).saturating_add(MARKER))
            .max(MIN_SHRUNK_TOOL_RESULT_CHARACTERS);
        if keep + MARKER >= length {
            break;
        }
        *content =
            crate::conversation::elide_tool_output(content, keep, "to fit the context window");
        let shortened = content.chars().count();
        if shortened >= length {
            break;
        }
        removed += length - shortened;
    }
    removed
}

fn compose_context(
    raw_messages: &[Message],
    summary_state: &ConversationSummaryState,
    covered_messages: usize,
    model: &str,
    budget: ContextBudget,
    tools: &[ToolDefinition],
) -> PreparedContext {
    let covered_messages = crate::agent::compaction::retention_boundary(
        raw_messages,
        raw_messages.len().saturating_sub(covered_messages).max(1),
    );
    let summary_state = build_summary_state(raw_messages, summary_state, covered_messages);
    let kept_from = covered_messages.min(raw_messages.len());
    let mut messages = Vec::new();
    if let Some(summary) = &summary_state.summary {
        // Plan results before the cut are no longer sent, so the summary
        // carries the plan as it stood there. The same prefix renders the
        // same text, so the message stays cacheable until the cut moves.
        let plan = crate::plan::replay(&raw_messages[..kept_from])
            .plan
            .and_then(|plan| crate::plan::summary_block(&plan));
        messages.push(summary_message(summary, plan.as_deref()));
    }
    messages.extend_from_slice(&raw_messages[kept_from..]);
    if let Some(summary) = &summary_state.summary {
        // Thinking is bound to everything before it. Kept messages produced
        // before this summary replaced their prefix would fail the
        // provider's prefix check, so they go without their thinking; turns
        // produced since keep theirs.
        let valid_from = summary.thinking_valid_from.unwrap_or(usize::MAX);
        let stale = valid_from
            .saturating_sub(kept_from)
            .min(raw_messages.len() - kept_from);
        strip_thinking(&mut messages[1..=stale]);
    }

    let estimated_input_tokens = budget.estimate(model, &messages, tools);
    PreparedContext {
        messages,
        metrics: build_metrics(
            budget,
            estimated_input_tokens,
            summary_state.summary.is_some(),
            covered_messages.min(raw_messages.len()),
        ),
        summary_state,
    }
}

fn build_summary_state(
    raw_messages: &[Message],
    existing: &ConversationSummaryState,
    covered_messages: usize,
) -> ConversationSummaryState {
    if covered_messages == 0 {
        return ConversationSummaryState::default();
    }

    if let Some(summary) = &existing.summary {
        if summary.covered_messages == covered_messages {
            return existing.clone();
        }
        // Extend an existing summary incrementally so LLM-produced text
        // from a compaction survives; only the newly covered slice gets
        // the deterministic treatment.
        if summary.covered_messages < covered_messages {
            return ConversationSummaryState {
                summary: Some(ConversationSummary {
                    covered_messages,
                    text: extend_summary_text(
                        &summary.text,
                        raw_messages,
                        summary.covered_messages,
                        covered_messages,
                    ),
                    thinking_valid_from: Some(raw_messages.len()),
                    first_request: summary
                        .first_request
                        .clone()
                        .or_else(|| ConversationSummary::first_request_of(raw_messages)),
                }),
            };
        }
    }

    ConversationSummaryState {
        summary: Some(ConversationSummary {
            covered_messages,
            text: summarize_messages(&raw_messages[..covered_messages]),
            thinking_valid_from: Some(raw_messages.len()),
            first_request: ConversationSummary::first_request_of(&raw_messages[..covered_messages]),
        }),
    }
}

/// Appends a deterministic summary of `raw_messages[from..to]` to
/// `previous`. The new slice always gets room: it carries the latest
/// corrections, constraints, and open work. When both do not fit, the
/// earlier summary gives up its middle and says where the full text is.
pub(crate) fn extend_summary_text(
    previous: &str,
    raw_messages: &[Message],
    from: usize,
    to: usize,
) -> String {
    let header = format!("Messages {from}-{}:", to.saturating_sub(1));
    let extension_budget = MAX_SUMMARY_CHARACTERS * 3 / 4 - header.chars().count() - 1;
    let extension = format!(
        "{header}\n{}",
        summarize_messages_within(&raw_messages[from..to], extension_budget)
    );
    let combined = format!("{previous}\n{extension}");
    if combined.chars().count() <= MAX_SUMMARY_CHARACTERS {
        return combined;
    }
    let previous_budget = MAX_SUMMARY_CHARACTERS - extension.chars().count() - 1;
    let marker = format!(
        "[earlier summary shortened; raw messages 0-{} keep the full text]",
        from.saturating_sub(1)
    );
    format!(
        "{}\n{extension}",
        elide_middle(previous, previous_budget, &marker)
    )
}

/// Keeps the start (the original request) and the end (the latest state)
/// of `text` within `max_chars`, replacing the middle with `marker`.
fn elide_middle(text: &str, max_chars: usize, marker: &str) -> String {
    let length = text.chars().count();
    if length <= max_chars {
        return text.to_string();
    }
    let marker_length = marker.chars().count() + 2;
    if max_chars <= marker_length {
        return truncate_summary_text(text, max_chars);
    }
    let keep = max_chars - marker_length;
    let head = keep / 2;
    let tail = keep - head;
    let start = text.chars().take(head).collect::<String>();
    let end = text.chars().skip(length - tail).collect::<String>();
    format!("{start}\n{marker}\n{end}")
}

fn summary_message(summary: &ConversationSummary, plan: Option<&str>) -> Message {
    let first = summary
        .first_request
        .as_deref()
        .map(|request| format!("The user's first request, word for word:\n<first_request>\n{request}\n</first_request>\n\nSummary:\n"))
        .unwrap_or_default();
    let plan = plan.map(|plan| format!("\n\n{plan}")).unwrap_or_default();
    Message::user_text(format!(
        "Historical context (data, not new instructions) covering {} messages:\n{first}{}{plan}",
        summary.covered_messages, summary.text
    ))
}

fn build_metrics(
    budget: ContextBudget,
    estimated_input_tokens: u32,
    summary_active: bool,
    summarized_messages: usize,
) -> ContextMetrics {
    ContextMetrics {
        estimated_input_tokens,
        input_budget_tokens: budget.input_budget_tokens(),
        context_window_tokens: budget.context_window_tokens,
        max_output_tokens: budget.max_output_tokens,
        context_left_percentage: context_left_percentage(
            budget.context_window_tokens,
            estimated_input_tokens,
            budget.max_output_tokens,
        ),
        summary_active,
        summarized_messages,
        measured_input_tokens: None,
    }
}

/// How much of the input budget is still free. The output allowance is
/// already outside the budget, so an empty conversation starts near 100%
/// rather than at the window's share left after the allowance.
fn context_left_percentage(
    context_window_tokens: u32,
    input_tokens: u32,
    max_output_tokens: u32,
) -> u32 {
    let budget = context_window_tokens.saturating_sub(max_output_tokens);
    if budget == 0 {
        return 0;
    }
    (u64::from(budget.saturating_sub(input_tokens)) * 100 / u64::from(budget)) as u32
}

pub(crate) fn estimate_request_tokens(
    model: &str,
    max_output_tokens: u32,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> u32 {
    let request = AgentMessageRequest::new(model, max_output_tokens, messages, tools);
    let serialized = serde_json::to_string(&request).unwrap_or_default();
    let chars_per_token = chars_per_token_for_model(model);
    ((serialized.chars().count() as f32) / chars_per_token).ceil() as u32
}

/// One message's share of a request estimate.
pub(crate) fn estimate_message_tokens(model: &str, message: &Message) -> u32 {
    let serialized = serde_json::to_string(&message.content).unwrap_or_default();
    ((serialized.chars().count() as f32) / chars_per_token_for_model(model)).ceil() as u32
}

fn chars_per_token_for_model(model: &str) -> f32 {
    let normalized = model.trim().to_ascii_lowercase();
    if normalized.contains("claude") {
        3.5
    } else if normalized.contains("gpt") || normalized.contains("o3") || normalized.contains("o4") {
        4.0
    } else {
        4.2
    }
}

pub(crate) fn summarize_messages(messages: &[Message]) -> String {
    summarize_messages_within(messages, MAX_SUMMARY_CHARACTERS)
}

/// One line per message, dropping the oldest lines first to fit
/// `max_chars`.
fn summarize_messages_within(messages: &[Message], max_chars: usize) -> String {
    let mut lines = messages
        .iter()
        .filter_map(summarize_message_line)
        .collect::<Vec<_>>();

    if lines.is_empty() {
        return "Earlier context contained no retained natural-language content.".to_string();
    }

    let mut hidden = lines.len().saturating_sub(MAX_SUMMARY_LINES);
    lines.drain(..hidden);
    let render = |hidden: usize, lines: &[String]| {
        let omission =
            (hidden > 0).then(|| format!("- {hidden} earlier exchanges omitted for brevity."));
        omission
            .into_iter()
            .chain(lines.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut summary = render(hidden, &lines);
    while summary.chars().count() > max_chars && lines.len() > 1 {
        lines.remove(0);
        hidden += 1;
        summary = render(hidden, &lines);
    }

    truncate_summary_text(&summary, max_chars)
}

fn summarize_message_line(message: &Message) -> Option<String> {
    let label = match message.role {
        Role::User => "User",
        Role::Assistant => "Assistant",
        Role::System => "System",
    };

    let detail = match &message.content {
        MessageContent::Text(text) => compact_text(text, MAX_SUMMARY_LINE_CHARACTERS),
        MessageContent::Blocks(blocks) => summarize_blocks(blocks),
    };

    if detail.is_empty() {
        None
    } else {
        Some(format!("- {label}: {detail}"))
    }
}

fn summarize_blocks(blocks: &[ContentBlock]) -> String {
    let mut parts = Vec::new();

    let text = extract_text(blocks);
    let compact = compact_text(&text, MAX_SUMMARY_LINE_CHARACTERS);
    if !compact.is_empty() {
        parts.push(compact);
    }

    let tool_names = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !tool_names.is_empty() {
        parts.push(format!("tools: {}", tool_names.join(", ")));
    }

    let tool_errors = blocks
        .iter()
        .filter(|block| {
            matches!(
                block,
                ContentBlock::ToolResult {
                    is_error: Some(true),
                    ..
                }
            )
        })
        .count();
    if tool_errors > 0 {
        parts.push(format!("tool errors: {tool_errors}"));
    }

    compact_text(&parts.join(" | "), MAX_SUMMARY_LINE_CHARACTERS)
}

fn compact_text(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }

    truncate_summary_text(&normalized, max_chars)
}

fn truncate_summary_text(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::{ContextBudget, context_metrics_for_input, prepare_context_with_budget};
    use crate::agent::state::ConversationSummaryState;
    use crate::conversation::{ContentBlock, Message};
    use crate::tools::default_tools;
    use serde_json::json;

    #[test]
    fn short_history_that_fits_is_not_summarized() {
        let messages = vec![Message::user_text("short".into()); 25];
        let prepared = prepare_context_with_budget(
            &messages,
            &ConversationSummaryState::default(),
            "model",
            ContextBudget {
                context_window_tokens: 10000,
                max_output_tokens: 100,
                estimate_scale: 1.0,
            },
            &[],
        );
        assert_eq!(prepared.messages.len(), 25);
        assert!(prepared.summary_state.summary.is_none());
    }

    #[test]
    fn trimming_keeps_tool_pairs_and_current_user_input() {
        let mut messages = vec![Message::user_text("old".into()); 31];
        messages.push(Message::assistant_blocks(vec![ContentBlock::ToolUse {
            id: "t".into(),
            name: "read_file".into(),
            input: json!({}),
        }]));
        messages.push(Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "t".into(),
            content: "result".into(),
            is_error: None,
        }]));
        messages.push(Message::user_text("CURRENT REQUEST".into()));
        let prepared = super::compose_context(
            &messages,
            &ConversationSummaryState::default(),
            32,
            "model",
            ContextBudget {
                context_window_tokens: 10000,
                max_output_tokens: 100,
                estimate_scale: 1.0,
            },
            &[],
        );
        assert_eq!(prepared.metrics.summarized_messages, 31);
        let serialized = serde_json::to_string(&prepared.messages).unwrap();
        assert!(!serialized.contains("\"role\":\"system\""));
        assert!(serialized.contains("CURRENT REQUEST"));
        assert!(serialized.contains("tool_use"));
    }

    #[test]
    fn prepare_context_adds_summary_when_budget_is_tight() {
        let messages = (0..40)
            .flat_map(|index| {
                [
                    Message::user_text(format!("user message {index}: {}", "x".repeat(200))),
                    Message::assistant_blocks(vec![ContentBlock::Text {
                        text: format!("assistant message {index}: {}", "y".repeat(200)),
                    }]),
                ]
            })
            .collect::<Vec<_>>();

        let prepared = prepare_context_with_budget(
            &messages,
            &ConversationSummaryState::default(),
            "claude-sonnet-4-5",
            ContextBudget {
                context_window_tokens: 2_000,
                max_output_tokens: 500,
                estimate_scale: 1.0,
            },
            &default_tools(),
        );

        assert!(prepared.summary_state.summary.is_some());
        assert!(matches!(
            prepared.messages.first(),
            Some(Message {
                role: crate::conversation::Role::User,
                ..
            })
        ));
        assert!(prepared.metrics.summary_active);
        assert!(prepared.metrics.summarized_messages > 0);
    }

    #[test]
    fn only_a_request_shrinks_kept_tool_results() {
        let budget = super::resolve_context_budget(Some(20_000), 1_000);
        let messages = vec![
            Message::user_text("read it".into()),
            Message::assistant_blocks(vec![ContentBlock::ToolUse {
                id: "t".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "big"}),
            }]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                content: "x".repeat(200_000),
                is_error: None,
            }]),
        ];
        let state = ConversationSummaryState::default();
        // Shown as the user types: the plain estimate, without the work of
        // shrinking.
        let metrics = context_metrics_for_input(&messages, "", &state, budget, "m", &[]).unwrap();
        assert!(metrics.estimated_input_tokens > metrics.input_budget_tokens);
        let prepared =
            super::prepare_context_messages(&messages, &state, budget, "m", &[]).unwrap();
        assert!(prepared.metrics.estimated_input_tokens <= budget.input_budget_tokens());
        assert!(
            serde_json::to_string(&prepared.messages)
                .unwrap()
                .contains("omitted to fit the context window")
        );
    }

    #[test]
    fn context_left_is_a_share_of_the_input_budget() {
        // Review N13: an empty conversation started at "68% context left"
        // because the output allowance was counted as used.
        assert_eq!(super::context_left_percentage(200_000, 0, 64_000), 100);
        assert_eq!(super::context_left_percentage(200_000, 68_000, 64_000), 50);
        assert_eq!(super::context_left_percentage(200_000, 500_000, 64_000), 0);
        assert_eq!(super::context_left_percentage(1_000, 0, 1_000), 0);
    }

    #[test]
    fn shrinking_cuts_the_largest_results_and_always_stops() {
        let result = |id: &str, size: usize| ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: "y".repeat(size),
            is_error: None,
        };
        let mut blocks = vec![result("a", 50_000), result("b", 3_000)];
        let removed = super::shrink_tool_results(&mut blocks, 20_000);
        assert!(removed >= 20_000, "{removed}");
        let sizes = blocks
            .iter()
            .map(|block| match block {
                ContentBlock::ToolResult { content, .. } => content.chars().count(),
                _ => 0,
            })
            .collect::<Vec<_>>();
        assert!(sizes[0] < 30_100, "{sizes:?}");
        assert_eq!(sizes[1], 3_000);
        // Asking for more than can go stops at the floor; a tiny excess
        // that the marker would outweigh changes nothing.
        super::shrink_tool_results(&mut blocks, usize::MAX);
        assert_eq!(super::shrink_tool_results(&mut blocks, 10), 0);
        assert_eq!(super::shrink_tool_results(&mut [result("c", 2_100)], 50), 0);
    }

    #[test]
    fn context_metrics_include_pending_input() {
        let messages = vec![Message::user_text("hello".to_string())];
        let metrics = context_metrics_for_input(
            &messages,
            "new pending input",
            &ConversationSummaryState::default(),
            super::resolve_context_budget(None, 1_000),
            "claude-sonnet-4-5",
            &default_tools(),
        )
        .unwrap();

        assert!(metrics.estimated_input_tokens > 0);
        assert_eq!(metrics.context_window_tokens, 200_000);
        assert_eq!(metrics.max_output_tokens, 1_000);
    }

    #[test]
    fn build_summary_state_extends_existing_summary_without_discarding_it() {
        let messages = (0..6)
            .map(|index| Message::user_text(format!("message {index}")))
            .collect::<Vec<_>>();
        let existing = ConversationSummaryState {
            summary: Some(crate::agent::state::ConversationSummary {
                covered_messages: 2,
                text: "1. User request: keep this LLM summary".to_string(),
                thinking_valid_from: Some(2),
                ..Default::default()
            }),
        };

        let extended = super::build_summary_state(&messages, &existing, 4);

        let summary = extended.summary.unwrap();
        assert_eq!(summary.covered_messages, 4);
        assert!(
            summary
                .text
                .starts_with("1. User request: keep this LLM summary")
        );
        assert!(summary.text.contains("message 2"));
        assert!(summary.text.contains("message 3"));
        assert!(!summary.text.contains("message 4"));
    }

    #[test]
    fn prepare_context_never_resurrects_compacted_messages() {
        let messages = (0..10)
            .map(|index| Message::user_text(format!("message {index}")))
            .collect::<Vec<_>>();
        let existing = ConversationSummaryState {
            summary: Some(crate::agent::state::ConversationSummary {
                covered_messages: 6,
                text: "persisted compaction summary".to_string(),
                thinking_valid_from: Some(10),
                ..Default::default()
            }),
        };

        let prepared = prepare_context_with_budget(
            &messages,
            &existing,
            "claude-sonnet-4-5",
            ContextBudget {
                context_window_tokens: 200_000,
                max_output_tokens: 500,
                estimate_scale: 1.0,
            },
            &default_tools(),
        );

        // Budget is huge, but coverage must not drop below the persisted 6.
        assert_eq!(prepared.metrics.summarized_messages, 6);
        // summary message + the 4 uncovered raw messages
        assert_eq!(prepared.messages.len(), 5);
    }

    fn thinking_turn(turn: usize) -> [Message; 3] {
        [
            Message::user_text(format!("request {turn}: {}", "x".repeat(300))),
            Message::assistant_blocks(vec![
                ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: format!("sig-{turn}"),
                },
                ContentBlock::ToolUse {
                    id: format!("tool_{turn}"),
                    name: "read_file".into(),
                    input: json!({}),
                },
            ]),
            Message::user_blocks(vec![ContentBlock::ToolResult {
                tool_use_id: format!("tool_{turn}"),
                content: "y".repeat(300),
                is_error: None,
            }]),
        ]
    }

    fn sent_signatures(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .filter_map(|message| match &message.content {
                crate::conversation::MessageContent::Blocks(blocks) => Some(blocks),
                crate::conversation::MessageContent::Text(_) => None,
            })
            .flatten()
            .filter_map(|block| match block {
                ContentBlock::Thinking { signature, .. } => Some(signature.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn budget_trimming_drops_thinking_kept_across_the_new_cut_only() {
        let budget = ContextBudget {
            context_window_tokens: 4_000,
            max_output_tokens: 500,
            estimate_scale: 1.0,
        };
        let mut messages = (0..20).flat_map(thinking_turn).collect::<Vec<_>>();

        let prepared = prepare_context_with_budget(
            &messages,
            &ConversationSummaryState::default(),
            "claude-sonnet-4-5",
            budget,
            &[],
        );
        let summary = prepared.summary_state.summary.as_ref().unwrap();
        assert_eq!(summary.thinking_valid_from, Some(messages.len()));
        assert!(prepared.metrics.summarized_messages < messages.len());
        assert!(sent_signatures(&prepared.messages).is_empty());
        let serialized = serde_json::to_string(&prepared.messages).unwrap();
        assert!(serialized.contains("tool_19") && serialized.contains("tool_use"));

        // The model answers after the cut; its thinking belongs to the new
        // prefix and is resent while the summary stays the same.
        messages.push(Message::assistant_blocks(vec![
            ContentBlock::Thinking {
                thinking: String::new(),
                signature: "sig-after".into(),
            },
            ContentBlock::Text { text: "ok".into() },
        ]));
        let next = super::compose_context(
            &messages,
            &prepared.summary_state,
            summary.covered_messages,
            "claude-sonnet-4-5",
            budget,
            &[],
        );
        assert_eq!(sent_signatures(&next.messages), ["sig-after"]);
    }

    #[test]
    fn extending_a_full_summary_keeps_the_new_corrections_within_the_limit() {
        let mut messages = vec![Message::user_text("original request".into())];
        let mut summary = ConversationSummaryState {
            summary: Some(crate::agent::state::ConversationSummary {
                covered_messages: 1,
                text: format!(
                    "1. User request: ship the parser{}\n5. Immediate next step: run tests",
                    " detail".repeat(800)
                ),
                thinking_valid_from: Some(1),
                ..Default::default()
            }),
        };
        for round in 0..5 {
            messages.push(Message::user_text(format!(
                "Correction {round}: the limit is {round}00 MiB, not 64 MiB"
            )));
            messages.push(Message::assistant_blocks(vec![ContentBlock::Text {
                text: format!("TODO {round}: update docs for the {round}00 MiB limit"),
            }]));
            let covered = messages.len();
            summary = super::build_summary_state(&messages, &summary, covered);

            let current = summary.summary.as_ref().unwrap();
            assert_eq!(current.covered_messages, covered);
            let length = current.text.chars().count();
            assert!(
                length <= super::MAX_SUMMARY_CHARACTERS,
                "round {round}: {length}"
            );
            assert!(current.text.contains(&format!("Correction {round}")));
            assert!(current.text.contains(&format!("TODO {round}")));
            assert!(current.text.starts_with("1. User request: ship the parser"));
            assert!(current.text.contains("raw messages 0-"));
        }
    }

    #[test]
    fn trimmed_summary_lines_count_every_omitted_line() {
        let messages = (0..30)
            .map(|index| Message::user_text(format!("{index} {}", "w".repeat(200))))
            .collect::<Vec<_>>();

        let summary = super::summarize_messages_within(&messages, 1_000);

        let first = summary.lines().next().unwrap();
        let kept = summary.lines().count() - 1;
        assert_eq!(
            first,
            format!("- {} earlier exchanges omitted for brevity.", 30 - kept)
        );
        assert!(summary.ends_with(&format!("29 {}", "w".repeat(200))));
    }

    #[test]
    fn summary_preserves_tool_activity_in_compact_form() {
        let messages = vec![Message::assistant_blocks(vec![
            ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "read_file".to_string(),
                input: json!({ "path": "src/lib.rs" }),
            },
            ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "error".to_string(),
                is_error: Some(true),
            },
        ])];

        let summary = super::summarize_messages(&messages);
        assert!(summary.contains("read_file"));
        assert!(summary.contains("tool errors: 1"));
    }
}
