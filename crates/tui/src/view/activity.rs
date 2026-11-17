//! Renders the Activity pane: one block of lines per item, scrolled so the
//! selected item, or the newest one when following, stays in view.

use crate::activity::{ActivityModel, ApprovalState, Item, ItemKind, RawView, ToolItem, ToolState};
use crate::view::cells::{cells, split_cells, take_cells};
use crate::view::text_layout::{command_output_preview_lines, wrap_multiline};
use crate::view::theme::*;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use socium_core::ThoughtRole;
use socium_core::providers::TokenUsage;

const GUTTER: usize = 2;
const SUBAGENT_GUIDE: &str = "│ ";
const COLLAPSED_THOUGHT_LINES: usize = 2;
const EXPANDED_INPUT_LINES: usize = 40;
const OK: Color = Color::Green;

/// Visible lines for a pane of `width` × `height` cells.
/// `focused` marks the selection.
pub(crate) fn activity_lines(
    model: &ActivityModel,
    focused: bool,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }
    let selected = model.selected_index().filter(|_| focused);
    let newest = model.items.len().checked_sub(1);
    let mut output_tokens = std::collections::HashMap::<usize, u32>::new();
    for item in &model.items {
        if let ItemKind::Round {
            stats: Some(stats), ..
        } = &item.kind
            && let Some(usage) = &stats.usage
        {
            *output_tokens.entry(item.turn_id).or_default() += usage.output_tokens;
        }
    }
    let mut lines = Vec::new();
    let mut selected_range = None;
    for (index, item) in model.items.iter().enumerate() {
        let start = lines.len();
        let is_selected = selected == Some(index);
        let live = newest == Some(index);
        let turn_output = output_tokens.get(&item.turn_id).copied();
        // A subagent's items hang under its `agent` call behind a guide.
        let guide = if item.agent.is_some() {
            SUBAGENT_GUIDE
        } else {
            ""
        };
        for (row, line) in item_lines(
            item,
            width.saturating_sub(GUTTER + cells(guide)).max(16),
            live,
            turn_output,
        )
        .into_iter()
        .enumerate()
        {
            let marker = if is_selected && row == 0 {
                "› "
            } else {
                "  "
            };
            let mut spans = vec![Span::styled(marker, Style::default().fg(ACCENT))];
            if !guide.is_empty() {
                spans.push(Span::styled(guide, Style::default().fg(MUTED)));
            }
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
        if is_selected {
            selected_range = Some((start, lines.len()));
        }
    }

    let max_start = lines.len().saturating_sub(height);
    let start = match (model.selected, selected_range) {
        // Keep the head of the selected item visible, with some context above.
        (Some(_), Some((first, end))) => {
            let preferred = first.saturating_sub(height / 3);
            let fits_end = end.saturating_sub(height);
            preferred.max(fits_end.min(first)).min(max_start)
        }
        _ => max_start,
    };
    lines.into_iter().skip(start).take(height).collect()
}

/// Visible lines of a round's raw stream, one event per entry with `ping`
/// keep-alives left out, scrolled `view.scroll` lines up from the end.
pub(crate) fn raw_lines(
    model: &ActivityModel,
    view: &RawView,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let Some(raw) = model.raw_round(view.turn_id, view.round) else {
        return vec![Line::from(Span::styled(
            "The raw stream of this round is no longer kept",
            Style::default().fg(MUTED),
        ))];
    };
    let inner = width.max(16);
    let mut lines = Vec::new();
    for json in raw.events.iter().filter(|json| !is_ping(json)) {
        for (row, line) in wrap_multiline(&sanitize(json), inner, inner.saturating_sub(2))
            .into_iter()
            .enumerate()
        {
            let lead = if row == 0 { "" } else { "  " };
            lines.push(Line::from(Span::styled(
                format!("{lead}{line}"),
                Style::default().fg(TEXT),
            )));
        }
    }
    if raw.truncated {
        lines.push(Line::from(Span::styled(
            "… later events exceeded the display limit",
            Style::default().fg(MUTED),
        )));
    }
    let end = lines.len().saturating_sub(view.scroll.min(lines.len()));
    let start = end.saturating_sub(height);
    lines.into_iter().skip(start).take(end - start).collect()
}

fn is_ping(json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json).is_ok_and(|v| v["type"] == "ping")
}

fn item_lines(
    item: &Item,
    width: usize,
    live: bool,
    turn_output: Option<u32>,
) -> Vec<Line<'static>> {
    match &item.kind {
        ItemKind::Turn {
            number,
            replayed,
            outcome,
        } => {
            let mut spans = vec![Span::styled(
                format!("━ turn {number}"),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            )];
            if *replayed {
                spans.push(Span::styled(
                    "  (earlier session)",
                    Style::default().fg(MUTED),
                ));
            }
            match outcome {
                Some(outcome) if outcome.ok => spans.push(Span::styled(
                    format!("  ✓ {}", format_ms(outcome.elapsed_ms)),
                    Style::default().fg(OK),
                )),
                Some(outcome) => spans.push(Span::styled(
                    format!("  ✗ {}", format_ms(outcome.elapsed_ms)),
                    Style::default().fg(ERROR),
                )),
                None if live || !*replayed => {
                    spans.push(Span::styled("  running…", Style::default().fg(MUTED)))
                }
                None => {}
            }
            if let Some(tokens) = turn_output {
                spans.push(Span::styled(
                    format!(" · out {}", format_count(tokens)),
                    Style::default().fg(MUTED),
                ));
            }
            let mut lines = vec![Line::from(spans)];
            if let Some(error) = outcome.as_ref().and_then(|o| o.error.as_deref()) {
                lines.extend(indented(error, width, "  ", Style::default().fg(ERROR)));
            }
            lines
        }
        ItemKind::Round { model, stats } => {
            let mut spans = vec![
                Span::styled(
                    format!("◆ round {}", item.round),
                    Style::default().fg(MUTED),
                ),
                Span::styled(format!("  {model}"), Style::default().fg(MUTED)),
            ];
            match stats {
                Some(stats) => {
                    let mut parts = Vec::new();
                    if let Some(reason) = &stats.stop_reason {
                        parts.push(reason.clone());
                    }
                    parts.push(format_usage(stats.usage.as_ref()));
                    if let Some(ttft) = stats.ttft_ms {
                        parts.push(format!("ttft {}", format_ms(ttft)));
                    }
                    parts.push(format_ms(stats.elapsed_ms));
                    spans.push(Span::styled(
                        format!("  {}", parts.join(" · ")),
                        Style::default().fg(MUTED),
                    ));
                }
                None => spans.push(Span::styled("  streaming…", Style::default().fg(MUTED))),
            }
            wrap_spans(spans, width)
        }
        ItemKind::Thought {
            role,
            text,
            truncated,
            interrupted,
            ..
        } => {
            let (icon, color, label) = match role {
                Some(ThoughtRole::Reasoning) => ("✦ ", ACCENT, "thinking"),
                Some(ThoughtRole::ProgressUpdate) => ("→ ", MUTED, "update"),
                None => ("· ", ACCENT, "thinking"),
            };
            let body = sanitize(text);
            let inner = width.saturating_sub(2);
            let mut wrapped = wrap_multiline(&body, inner, inner);
            let collapse = !item.expanded && role.is_some() && !live;
            let hidden = wrapped.len().saturating_sub(COLLAPSED_THOUGHT_LINES);
            if collapse && hidden > 0 {
                wrapped.truncate(COLLAPSED_THOUGHT_LINES);
            }
            let mut lines = Vec::new();
            for (row, line) in wrapped.into_iter().enumerate() {
                let prefix = if row == 0 { icon } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(color)),
                    Span::styled(line, Style::default().fg(TEXT)),
                ]));
            }
            if lines.is_empty() {
                lines.push(Line::from(Span::styled(icon, Style::default().fg(color))));
            }
            let mut notes = Vec::new();
            if collapse && hidden > 0 {
                notes.push(format!("+{hidden} lines"));
            }
            if *truncated {
                notes.push("display limit reached".into());
            }
            if *interrupted {
                notes.push("interrupted".into());
            }
            if role.is_none() && live {
                notes.push(format!("{label}…"));
            }
            if !notes.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", notes.join(" · ")),
                    Style::default().fg(MUTED),
                )));
            }
            lines
        }
        ItemKind::Response { chars } => vec![Line::from(Span::styled(
            format!("✎ response · {chars} chars"),
            Style::default().fg(MUTED),
        ))],
        ItemKind::Tool(tool) => tool_lines(tool, item.expanded, width),
        ItemKind::Note(text) => indented(&sanitize(text), width, "• ", Style::default().fg(MUTED)),
        ItemKind::Signal(text) => indented(
            &sanitize(text),
            width,
            "◇ ",
            Style::default().fg(ACCENT).add_modifier(Modifier::ITALIC),
        ),
    }
}

fn tool_lines(tool: &ToolItem, expanded: bool, width: usize) -> Vec<Line<'static>> {
    let awaiting_approval = matches!(tool.approval, Some(ApprovalState::Waiting));
    let (status, status_color) = match &tool.state {
        ToolState::Composing => ("writing input…".to_string(), MUTED),
        ToolState::Ready | ToolState::Running if awaiting_approval => {
            ("awaiting approval".to_string(), ACCENT)
        }
        ToolState::Ready => ("queued".to_string(), MUTED),
        ToolState::Running => match tool.output_bytes {
            Some((out, err)) => (
                format!(
                    "running · stdout {} · stderr {}",
                    format_bytes(out),
                    format_bytes(err)
                ),
                MUTED,
            ),
            None => ("running…".to_string(), MUTED),
        },
        ToolState::Finished {
            is_error,
            elapsed_ms,
            truncated,
            ..
        } => (
            format!(
                "{} {}{}",
                if *is_error { "✗" } else { "✓" },
                format_ms(*elapsed_ms),
                if *truncated { " · cut" } else { "" }
            ),
            if *is_error { ERROR } else { OK },
        ),
    };
    let mut spans = vec![
        Span::styled("⚙ ", Style::default().fg(MUTED)),
        Span::styled(tool.name.clone(), Style::default().fg(TEXT)),
    ];
    if !tool.input_summary.is_empty() {
        spans.push(Span::styled(
            format!("  {}", sanitize(&tool.input_summary)),
            Style::default().fg(TEXT),
        ));
    }
    if tool.state == ToolState::Composing {
        spans.push(Span::styled("▍", Style::default().fg(ACCENT)));
    }
    spans.push(Span::styled(
        format!("  {status}"),
        Style::default().fg(status_color),
    ));
    let mut lines = wrap_spans(spans, width);

    match &tool.approval {
        Some(ApprovalState::Waiting) => lines.push(Line::from(Span::styled(
            "  ⏸ waiting for your approval (y / n)",
            Style::default().fg(ACCENT),
        ))),
        Some(ApprovalState::Resolved {
            approved,
            waited_ms,
        }) => lines.push(Line::from(Span::styled(
            format!(
                "  {} after {}",
                if *approved {
                    "approved"
                } else {
                    "not approved"
                },
                format_ms(*waited_ms)
            ),
            Style::default().fg(if *approved { MUTED } else { ERROR }),
        ))),
        None => {}
    }
    if let ToolState::Finished {
        is_error, summary, ..
    } = &tool.state
        && !summary.is_empty()
    {
        lines.extend(indented(
            summary,
            width,
            "  ↳ ",
            Style::default().fg(if *is_error { ERROR } else { MUTED }),
        ));
    }

    if expanded {
        let inner = width.saturating_sub(4);
        if !tool.partial_json.is_empty() {
            lines.push(Line::from(Span::styled(
                "  input:",
                Style::default().fg(MUTED),
            )));
            let text = serde_json::from_str::<serde_json::Value>(&tool.partial_json)
                .map_or_else(|_| tool.partial_json.clone(), |value| input_text(&value));
            for line in wrap_keeping_indent(&sanitize(&text), inner)
                .into_iter()
                .take(EXPANDED_INPUT_LINES)
            {
                lines.push(Line::from(Span::styled(
                    format!("    {line}"),
                    Style::default().fg(TEXT),
                )));
            }
        }
        if let Some(output) = &tool.command_output {
            for line in command_output_preview_lines(output, inner, EXPANDED_INPUT_LINES) {
                lines.push(Line::from(Span::styled(
                    format!("    {}", sanitize(&line)),
                    Style::default().fg(TEXT),
                )));
            }
        }
    }
    lines
}

/// Tool input as `key: value` lines, with string values shown as written
/// so commands and file contents keep their line breaks.
fn input_text(input: &serde_json::Value) -> String {
    let Some(fields) = input.as_object() else {
        return serde_json::to_string_pretty(input).unwrap_or_default();
    };
    let mut out = Vec::new();
    for (key, value) in fields {
        match value {
            serde_json::Value::Array(items) => {
                out.push(format!("{key}:"));
                for item in items {
                    push_value(&mut out, "  - ", "    ", item);
                }
            }
            serde_json::Value::String(text) if text.contains('\n') => {
                out.push(format!("{key}:"));
                push_value(&mut out, "  ", "  ", value);
            }
            _ => push_value(&mut out, &format!("{key}: "), "  ", value),
        }
    }
    out.join("\n")
}

/// Wraps each line on its own, keeping its leading indentation on every
/// wrapped row.
fn wrap_keeping_indent(text: &str, width: usize) -> Vec<String> {
    text.lines()
        .flat_map(|line| {
            let body = line.trim_start();
            let indent = &line[..line.len() - body.len()];
            if body.is_empty() {
                return vec![String::new()];
            }
            let inner = width.saturating_sub(cells(indent)).max(1);
            wrap_multiline(body, inner, inner)
                .into_iter()
                .map(|row| format!("{indent}{row}"))
                .collect()
        })
        .collect()
}

fn push_value(out: &mut Vec<String>, lead: &str, rest: &str, value: &serde_json::Value) {
    let text = match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    for (row, line) in text.lines().enumerate() {
        out.push(format!("{}{line}", if row == 0 { lead } else { rest }));
    }
}

fn indented(text: &str, width: usize, prefix: &'static str, style: Style) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(cells(prefix));
    wrap_multiline(text, inner, inner)
        .into_iter()
        .enumerate()
        .map(|(row, line)| {
            let lead = if row == 0 {
                prefix.to_string()
            } else {
                " ".repeat(cells(prefix))
            };
            Line::from(vec![Span::styled(lead, style), Span::styled(line, style)])
        })
        .collect()
}

/// Wraps a styled line at word boundaries, keeping each span's style.
/// Widths are terminal cells; a word wider than the pane, such as unspaced
/// Japanese text, is split between graphemes instead of running off it.
fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    const CONTINUATION: &str = "  ";
    let width = width.max(CONTINUATION.len() + 1);
    let mut lines = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for span in spans {
        let style = span.style;
        let mut pending = String::new();
        for word in split_keep_spaces(&span.content) {
            let word_width = cells(word);
            if used + word_width <= width {
                pending.push_str(word);
                used += word_width;
                continue;
            }
            let mut rest = word;
            if used > 0 {
                if !pending.is_empty() {
                    current.push(Span::styled(std::mem::take(&mut pending), style));
                }
                lines.push(Line::from(std::mem::take(&mut current)));
                pending.push_str(CONTINUATION);
                used = CONTINUATION.len();
                rest = word.trim_start();
            }
            let (first, first_width) = take_cells(rest, width - used);
            pending.push_str(&first);
            used += first_width;
            let remainder = &rest[first.len()..];
            if remainder.is_empty() {
                continue;
            }
            for piece in split_cells(remainder, width - CONTINUATION.len()) {
                current.push(Span::styled(std::mem::take(&mut pending), style));
                lines.push(Line::from(std::mem::take(&mut current)));
                used = CONTINUATION.len() + cells(&piece);
                pending.push_str(CONTINUATION);
                pending.push_str(&piece);
            }
        }
        if !pending.is_empty() {
            current.push(Span::styled(pending, style));
        }
    }
    if !current.is_empty() {
        lines.push(Line::from(current));
    }
    lines
}

/// Splits text into words that each carry their leading spaces.
fn split_keep_spaces(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_word = false;
    for (index, c) in text.char_indices() {
        if c == ' ' && in_word {
            parts.push(&text[start..index]);
            start = index;
            in_word = false;
        } else if c != ' ' {
            in_word = true;
        }
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    parts
}

/// Removes terminal control characters while keeping line breaks.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

fn format_usage(usage: Option<&TokenUsage>) -> String {
    let Some(usage) = usage else {
        return "usage n/a".into();
    };
    let cached = usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
    let mut text = format!("in {}", format_count(usage.total_input_tokens()));
    if cached > 0 {
        text.push_str(&format!(" (cache {})", format_count(cached)));
    }
    text.push_str(&format!(" · out {}", format_count(usage.output_tokens)));
    text
}

fn format_count(value: u32) -> String {
    if value < 1_000 {
        value.to_string()
    } else {
        format!("{:.1}k", f64::from(value) / 1_000.0)
    }
}

fn format_bytes(value: u64) -> String {
    match value {
        0..1_024 => format!("{value} B"),
        1_024..1_048_576 => format!("{:.1} KiB", value as f64 / 1_024.0),
        _ => format!("{:.1} MiB", value as f64 / 1_048_576.0),
    }
}

pub(crate) fn format_ms(ms: u64) -> String {
    match ms {
        0..1_000 => format!("{ms}ms"),
        1_000..60_000 => format!("{:.1}s", ms as f64 / 1_000.0),
        _ => format!("{}m{:02}s", ms / 60_000, (ms % 60_000) / 1_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::tests::{model_with, sample_turn};

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn renders_turn_rounds_thoughts_tools_and_stats() {
        let model = model_with(sample_turn());
        let rows = plain(&activity_lines(&model, false, 100, 50));

        assert_eq!(rows[0], "  ━ turn 1  ✓ 1.5s · out 412");
        assert_eq!(
            rows[1],
            "  ◆ round 1  claude-opus-5-5  tool_use · in 18.0k (cache 16.0k) · out 412 · ttft 1.2s · 6.8s"
        );
        assert_eq!(rows[2], "  → Check how the view draws activity.");
        assert_eq!(rows[3], "  ⚙ read_file  crates/tui/src/view/mod.rs  ✗ 12ms");
        assert_eq!(rows[4], "    ↳ File not found");
        assert!(rows[5].contains("round 2") && rows[5].contains("usage n/a"));
        assert_eq!(rows[6], "  ✦ The path was wrong; answer from what I know.");
        assert_eq!(rows[7], "  ✎ response · 312 chars");
    }

    #[test]
    fn follows_the_newest_lines_and_scrolls_to_the_selection() {
        let mut model = model_with(sample_turn());
        let tail = plain(&activity_lines(&model, false, 100, 2));
        assert_eq!(tail.last().unwrap(), "  ✎ response · 312 chars");

        model.select_first();
        let head = plain(&activity_lines(&model, true, 100, 2));
        assert_eq!(head[0], "› ━ turn 1  ✓ 1.5s · out 412");
    }

    #[test]
    fn finished_thoughts_collapse_until_expanded() {
        let mut events = sample_turn();
        if let socium_core::ActivityKind::ThoughtDelta { text, .. } = &mut events[3].kind {
            *text = "one two three four five six seven eight nine ten".repeat(4);
        }
        let mut model = model_with(events);
        let collapsed = plain(&activity_lines(&model, false, 40, 80));
        assert!(collapsed.iter().any(|row| row.contains("lines")));

        model.selected = Some(2);
        model.toggle_expanded();
        let expanded = plain(&activity_lines(&model, true, 40, 80));
        assert!(expanded.len() > collapsed.len());
        assert!(!expanded.iter().any(|row| row.contains("+")));
    }

    #[test]
    fn tool_rows_wrap_wide_text_by_cells_without_losing_it() {
        use ratatui::style::Style;
        use ratatui::text::Span;
        let path = "ドキュメント/設計書/表示幅の検証👨\u{200d}👩\u{200d}👧.md";
        let spans = vec![
            Span::styled("  ⚙ read_file  ", Style::default()),
            Span::styled(path.to_string(), Style::default()),
            Span::styled("  ✓ 3ms", Style::default()),
        ];

        let lines = super::wrap_spans(spans, 20);
        let rows = plain(&lines);

        assert!(lines.iter().all(|line| line.width() <= 20), "{rows:#?}");
        let joined = rows.iter().map(|row| row.trim_start()).collect::<String>();
        assert!(joined.contains(path), "{joined}");
        assert!(joined.ends_with("✓ 3ms"));
    }

    #[test]
    fn model_text_cannot_inject_terminal_escapes() {
        let mut events = sample_turn();
        if let socium_core::ActivityKind::ThoughtDelta { text, .. } = &mut events[3].kind {
            *text = "safe\u{1b}]52;c;bad\u{7}".into();
        }
        let rows = plain(&activity_lines(&model_with(events), false, 100, 50));
        assert!(rows.iter().all(|row| !row.chars().any(|c| c.is_control())));
    }

    #[test]
    fn raw_view_lists_events_without_pings_and_scrolls_from_the_end() {
        let mut model = model_with(sample_turn());
        for json in [
            r#"{"type":"message_start"}"#,
            r#"{"type":"ping"}"#,
            "{\"type\":\"content_block_delta\",\"x\":\"\u{1b}[2J\"}",
            r#"{"type":"message_stop"}"#,
        ] {
            model.push_raw(4, 1, json.into());
        }
        let view = RawView {
            turn_id: 4,
            round: 1,
            scroll: 0,
        };
        let rows = plain(&raw_lines(&model, &view, 80, 10));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], r#"{"type":"message_start"}"#);
        assert!(rows.iter().all(|row| !row.chars().any(char::is_control)));

        let scrolled = RawView { scroll: 1, ..view };
        let rows = plain(&raw_lines(&model, &scrolled, 80, 10));
        assert_eq!(rows.last().unwrap(), &rows[1]);
        assert!(!rows.iter().any(|row| row.contains("message_stop")));
    }

    fn command_awaiting_approval() -> ActivityModel {
        use crate::activity::tests::event;
        use socium_core::ActivityKind;
        let input =
            r#"{"command":["sh","-c","cat <<'EOF'\nfirst line\n\nEOF"],"timeout_ms":120000}"#;
        model_with(vec![
            event(1, 0, ActivityKind::TurnStarted),
            event(1, 1, ActivityKind::RoundStarted { model: "m".into() }),
            event(
                1,
                1,
                ActivityKind::ToolIntentStarted {
                    block: 0,
                    call_id: "c".into(),
                    name: "run_command".into(),
                },
            ),
            event(
                1,
                1,
                ActivityKind::ToolIntentDelta {
                    block: 0,
                    partial_json: input.into(),
                },
            ),
            event(1, 1, ActivityKind::ToolIntentFinished { block: 0 }),
            event(
                1,
                1,
                ActivityKind::ToolStarted {
                    call_id: "c".into(),
                    name: "run_command".into(),
                    input_summary: "sh -c cat <<'EOF' first line EOF".into(),
                },
            ),
            event(
                1,
                1,
                ActivityKind::ApprovalWaiting {
                    call_id: "c".into(),
                    tool: "run_command".into(),
                },
            ),
        ])
    }

    #[test]
    fn a_cut_result_says_so_on_its_tool_line() {
        let mut events = sample_turn();
        for event in &mut events {
            if let socium_core::ActivityKind::ToolFinished { truncated, .. } = &mut event.kind {
                *truncated = true;
            }
        }
        let rows = plain(&activity_lines(&model_with(events), false, 100, 50));
        let tool_row = rows.iter().find(|row| row.contains("read_file")).unwrap();
        assert!(tool_row.ends_with("✗ 12ms · cut"), "{tool_row}");
    }

    #[test]
    fn a_tool_waiting_for_approval_is_not_shown_as_running() {
        let model = command_awaiting_approval();
        let rows = plain(&activity_lines(&model, false, 100, 50));
        let tool_row = rows.iter().find(|row| row.contains("run_command")).unwrap();

        assert!(tool_row.ends_with("  awaiting approval"), "{tool_row}");
        assert!(!tool_row.contains("running"));
    }

    #[test]
    fn expanded_input_shows_string_values_with_real_line_breaks() {
        let mut model = command_awaiting_approval();
        model.select_last();
        model.toggle_expanded();
        let rows = plain(&activity_lines(&model, true, 100, 50));
        let input = rows
            .iter()
            .skip_while(|row| row.trim() != "input:")
            .skip(1)
            .map(String::as_str)
            .collect::<Vec<_>>();

        assert_eq!(
            input,
            [
                "      command:",
                "        - sh",
                "        - -c",
                "        - cat <<'EOF'",
                "          first line",
                "      ",
                "          EOF",
                "      timeout_ms: 120000",
            ]
        );
    }

    #[test]
    fn durations_and_sizes_are_compact() {
        assert_eq!(format_ms(42), "42ms");
        assert_eq!(format_ms(6_800), "6.8s");
        assert_eq!(format_ms(125_000), "2m05s");
        assert_eq!(format_bytes(2_048), "2.0 KiB");
        assert_eq!(format_count(999), "999");
    }

    #[test]
    fn subagent_items_hang_under_a_guide() {
        let mut model = ActivityModel::default();
        for event in crate::activity::tests::turn_with_subagent() {
            model.apply(event);
        }

        let lines = plain(&activity_lines(&model, false, 60, 40));

        let guided = lines
            .iter()
            .filter(|line| line.starts_with("  │ "))
            .cloned()
            .collect::<Vec<_>>();
        // Every line of a subagent item, wrapped ones included.
        assert_eq!(guided.len(), 3, "{lines:#?}");
        assert!(guided[0].contains("◆ round 1"), "{guided:#?}");
        assert!(guided[1].contains("look in a.txt"), "{guided:#?}");
        assert!(guided[2].contains("thinking…"), "{guided:#?}");
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("  ⚙ agent  Find the greeting")),
            "{lines:#?}"
        );
    }

    #[test]
    fn concurrent_subagents_hang_under_their_own_agent_calls() {
        let events = crate::activity::tests::turn_with_concurrent_subagents();
        let live = model_with(events.clone());
        let mut replayed = ActivityModel::default();
        for event in events {
            replayed.apply_replayed(event);
        }

        let rows = plain(&activity_lines(&live, false, 80, 40));
        assert_eq!(
            rows[2..],
            [
                "  ⚙ agent  Scan src  ✓ 5ms",
                "    ↳ src done",
                "  │ ◆ round 1  m  streaming…",
                "  │ ⚙ read_file  src/lib.rs  ✓ 5ms",
                "  │   ↳ lib read",
                "  ⚙ agent  Scan docs  ✓ 5ms",
                "    ↳ docs done",
                "  │ ◆ round 1  m  streaming…",
                "  │ ⚙ read_file  docs/guide.md  ✓ 5ms",
                "  │   ↳ guide read",
            ],
            "{rows:#?}"
        );
        assert_eq!(
            plain(&activity_lines(&replayed, false, 80, 40))[1..],
            rows[1..]
        );
    }
}
