use crate::transcript::{ChatEntry, CommandOutputState, Speaker};
use crate::view::cells::{cells, take_cells, truncate_cells, wrap_words};
use crate::view::theme::*;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

enum Segment {
    Text(String),
    Code { label: String, lines: Vec<String> },
}

pub(crate) fn command_output_preview_lines(
    output: &CommandOutputState,
    width: usize,
    max_lines: usize,
) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }

    let wrap_width = width.max(12);
    let mut lines = vec![
        format!("$ {}", output.command),
        if output.timed_out {
            "status: timed out".to_string()
        } else {
            match output.exit_code {
                Some(code) => format!("status: exit {code}"),
                None => "status: terminated".to_string(),
            }
        },
    ];

    lines.push("stdout:".to_string());
    lines.extend(wrap_multiline(
        &output.stdout_preview,
        wrap_width,
        wrap_width,
    ));
    lines.push("stderr:".to_string());
    lines.extend(wrap_multiline(
        &output.stderr_preview,
        wrap_width,
        wrap_width,
    ));

    lines.into_iter().take(max_lines).collect()
}

pub(crate) fn help_text() -> String {
    format!(
        "{}\n\nexit           Quit Socium (same as /exit)\n/              Show command suggestions; Up/Down select, Tab completes\nEsc / Ctrl+C   Cancel a running task\nApproval       Up/Down and Enter, 1/2, or y/n to allow once or deny;\n               PgUp/PgDn, Home/End, Space/b scroll the request\nCtrl+A         Focus the activity pane; Up/Down select, Ctrl+L goes to\n               the top, Enter expands, z fills the screen, Esc returns\nSandbox        Linux; network denied; private HOME; no host fallback",
        crate::slash_command::command_list_text()
    )
}

pub(crate) fn slice_chars(value: &str, start: usize, end: usize) -> String {
    value
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

pub(crate) fn render_entry(entry: &ChatEntry, width: usize) -> Vec<Line<'static>> {
    match entry.speaker {
        Speaker::User => render_wrapped_text(&entry.body, width, "> ", "  ", MUTED, MUTED, None),
        Speaker::Socium => render_socium_entry(entry, width),
        Speaker::Thinking => render_thinking_indicator(),
        Speaker::System => render_wrapped_text(&entry.body, width, "! ", "  ", PINK, TEXT, None),
        Speaker::Tool { failed } => render_tool_entry(&entry.body, width, failed),
    }
}

fn render_thinking_indicator() -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled("✦ ", Style::default().fg(LILAC)),
            Span::styled("Socium is thinking", Style::default().fg(LILAC)),
            Span::styled(
                " · Ctrl+A to follow in activity · esc to cancel",
                Style::default().fg(MUTED),
            ),
        ]),
        Line::from(""),
    ]
}

fn render_socium_entry(entry: &ChatEntry, width: usize) -> Vec<Line<'static>> {
    let segments = parse_segments(&entry.body);
    let mut lines = Vec::new();
    let mut wrote_text = false;

    for segment in segments {
        match segment {
            Segment::Text(text) => {
                let wrapped =
                    wrap_multiline(&text, width.saturating_sub(2), width.saturating_sub(2));
                if wrapped.is_empty() {
                    continue;
                }

                if !wrote_text {
                    let first = wrapped.first().cloned().unwrap_or_default();
                    lines.push(Line::from(vec![
                        Span::styled("✦ ", Style::default().fg(LILAC)),
                        Span::styled(first, Style::default().fg(TEXT)),
                    ]));
                    for line in wrapped.into_iter().skip(1) {
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default().fg(MUTED)),
                            Span::styled(line, Style::default().fg(TEXT)),
                        ]));
                    }
                    wrote_text = true;
                } else {
                    for line in wrapped {
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default().fg(MUTED)),
                            Span::styled(line, Style::default().fg(TEXT)),
                        ]));
                    }
                }
            }
            Segment::Code { label, lines: code } => {
                lines.extend(render_code_block(&label, &code, width));
                wrote_text = true;
            }
        }
    }

    if !wrote_text {
        lines.push(Line::from(Span::styled("✦", Style::default().fg(LILAC))));
    }

    lines.push(Line::from(""));
    lines
}

/// `⏺ name argument`, then `⎿ result` under it, the marker pink when the
/// call failed.
fn render_tool_entry(body: &str, width: usize, failed: bool) -> Vec<Line<'static>> {
    let (call, result) = body.split_once('\n').unwrap_or((body, ""));
    let marker = if failed { PINK } else { LILAC };
    let mut lines = render_wrapped_text(call, width, "⏺ ", "  ", marker, TEXT, None);
    if !result.is_empty() {
        lines.pop();
        let color = if failed { PINK } else { MUTED };
        lines.extend(render_wrapped_text(
            result, width, "  ⎿ ", "    ", MUTED, color, None,
        ));
    }
    lines
}

fn render_wrapped_text(
    text: &str,
    width: usize,
    prefix: &str,
    indent: &str,
    prefix_color: Color,
    text_color: Color,
    timestamp: Option<&str>,
) -> Vec<Line<'static>> {
    let wrapped = wrap_multiline(
        text,
        width.saturating_sub(cells(prefix)),
        width.saturating_sub(cells(indent)),
    );
    let mut lines = Vec::new();

    if let Some(first) = wrapped.first() {
        let mut first_line = vec![
            Span::styled(prefix.to_string(), Style::default().fg(prefix_color)),
            Span::styled(first.clone(), Style::default().fg(text_color)),
        ];
        if let Some(ts) = timestamp {
            first_line.push(Span::styled(format!("  {ts}"), Style::default().fg(SOFT)));
        }
        lines.push(Line::from(first_line));
    }

    for line in wrapped.into_iter().skip(1) {
        lines.push(Line::from(vec![
            Span::styled(indent.to_string(), Style::default().fg(MUTED)),
            Span::styled(line, Style::default().fg(text_color)),
        ]));
    }

    lines.push(Line::from(""));
    lines
}

fn parse_segments(body: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut text_buffer = Vec::new();
    let mut code_buffer = Vec::new();
    let mut code_label = String::new();
    let mut in_code = false;

    for line in body.lines() {
        if let Some(info) = line.strip_prefix("```") {
            if in_code {
                segments.push(Segment::Code {
                    label: if code_label.is_empty() {
                        "snippet".to_string()
                    } else {
                        code_label.clone()
                    },
                    lines: code_buffer.clone(),
                });
                code_buffer.clear();
                code_label.clear();
                in_code = false;
            } else {
                if !text_buffer.is_empty() {
                    segments.push(Segment::Text(text_buffer.join("\n")));
                    text_buffer.clear();
                }
                code_label = info.trim().to_string();
                in_code = true;
            }
            continue;
        }

        if in_code {
            code_buffer.push(line.to_string());
        } else {
            text_buffer.push(line.to_string());
        }
    }

    if !text_buffer.is_empty() {
        segments.push(Segment::Text(text_buffer.join("\n")));
    }

    if in_code {
        segments.push(Segment::Code {
            label: if code_label.is_empty() {
                "snippet".to_string()
            } else {
                code_label
            },
            lines: code_buffer,
        });
    }

    if segments.is_empty() {
        segments.push(Segment::Text(body.to_string()));
    }

    segments
}

pub(crate) fn wrap_multiline(text: &str, first_width: usize, next_width: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut current_width = first_width.max(1);

    for paragraph in text.lines() {
        if paragraph.is_empty() {
            result.push(String::new());
            current_width = next_width.max(1);
            continue;
        }

        let wrapped = wrap_paragraph(paragraph, current_width, next_width.max(1));
        result.extend(wrapped);
        current_width = next_width.max(1);
    }

    result
}

fn wrap_paragraph(paragraph: &str, first_width: usize, next_width: usize) -> Vec<String> {
    wrap_words(paragraph, first_width, next_width)
}

fn render_code_block(label: &str, code_lines: &[String], width: usize) -> Vec<Line<'static>> {
    let outer_width = width.saturating_sub(2).max(24);
    let inner_width = outer_width.saturating_sub(2);
    let title = format!(" {} ", if label.is_empty() { "snippet" } else { label });
    let (title_text, title_width) = take_cells(&title, inner_width);
    let fill = inner_width.saturating_sub(title_width);

    let mut lines = vec![Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled(
            format!("┌{}{}┐", title_text, "─".repeat(fill)),
            Style::default().fg(CODE_BORDER),
        ),
    ])];

    // Long lines continue on the next row instead of being cut off.
    let rows = code_lines.iter().flat_map(|raw_line| {
        crate::view::cells::wrap_preformatted(raw_line, inner_width.saturating_sub(2))
            .into_iter()
            .chain(raw_line.is_empty().then(String::new))
    });
    for visible in rows {
        let padding = inner_width.saturating_sub(2 + cells(&visible));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().fg(MUTED)),
            Span::styled("│ ", Style::default().fg(CODE_BORDER)),
            Span::styled(visible, Style::default().fg(CODE_TEXT)),
            Span::styled(
                format!("{} │", " ".repeat(padding)),
                Style::default().fg(CODE_BORDER),
            ),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled(
            format!("└{}┘", "─".repeat(inner_width)),
            Style::default().fg(CODE_BORDER),
        ),
    ]));
    lines.push(Line::from(""));

    lines
}

/// `text` cut to `max_cells` terminal cells, ending with `…` when cut.
pub(crate) fn truncate_text(text: &str, max_cells: usize) -> String {
    truncate_cells(text, max_cells)
}

#[cfg(test)]
mod tests {
    use super::{command_output_preview_lines, render_code_block};
    use crate::transcript::CommandOutputState;

    #[test]
    fn help_text_lists_supported_slash_commands() {
        let help = super::help_text();

        assert!(help.contains("/help"));
        assert!(help.contains("/clear"));
        assert!(help.contains("/exit"));
        assert!(help.contains("/commands"));
    }

    #[test]
    fn command_output_preview_lines_include_status_and_streams() {
        let output = CommandOutputState {
            command: "cargo test".to_string(),
            exit_code: Some(0),
            timed_out: false,
            stdout_preview: "ok".to_string(),
            stderr_preview: "(empty)".to_string(),
        };

        let lines = command_output_preview_lines(&output, 40, 6);

        assert_eq!(lines[0], "$ cargo test");
        assert_eq!(lines[1], "status: exit 0");
        assert!(lines.contains(&"stdout:".to_string()));
        assert!(lines.contains(&"stderr:".to_string()));
    }

    #[test]
    fn code_block_borders_align_with_wide_characters() {
        let code = [
            "search_text  → (変化なし)".to_string(),
            "ascii only".to_string(),
            "全角".repeat(40),
        ];
        let lines = render_code_block("日本語の見出し", &code, 40);
        let boxed = &lines[..lines.len() - 1];

        assert!(boxed.iter().all(|line| line.width() == 40), "{boxed:#?}");
    }

    fn row_text(line: &ratatui::text::Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn long_code_lines_wrap_instead_of_losing_their_end() {
        let code = [format!("let 説明 = \"{}\"; // 終端", "日本語".repeat(10))];
        let lines = render_code_block("rust", &code, 30);
        let body = lines[1..lines.len() - 2]
            .iter()
            .map(|line| row_text(line))
            .collect::<Vec<_>>();

        assert!(body.len() > 1);
        let boxed = &lines[..lines.len() - 1];
        assert!(boxed.iter().all(|line| line.width() == 30), "{boxed:#?}");
        let joined = body
            .iter()
            .map(|row| {
                row.trim_start_matches("  │ ")
                    .trim_end_matches(" │")
                    .trim_end()
            })
            .collect::<String>();
        assert!(joined.ends_with("// 終端"), "{joined}");
    }

    #[test]
    fn conversation_wraps_by_cells_without_dropping_japanese_text() {
        let entry = crate::transcript::ChatEntry {
            speaker: crate::transcript::Speaker::System,
            body:
                "差分を確認してから承認してください。e\u{301}と👨\u{200d}👩\u{200d}👧も表示します。"
                    .into(),
        };
        let lines = super::render_entry(&entry, 24);
        let rows = lines.iter().map(row_text).collect::<Vec<_>>();

        assert!(lines.iter().all(|line| line.width() <= 24), "{rows:#?}");
        let joined = rows
            .iter()
            .map(|row| row.trim_start_matches("! ").trim_start())
            .collect::<String>();
        assert_eq!(
            joined,
            "差分を確認してから承認してください。e\u{301}と👨\u{200d}👩\u{200d}👧も表示します。"
        );
    }

    #[test]
    fn a_tool_entry_shows_the_call_then_its_result() {
        let entry = crate::transcript::ChatEntry {
            speaker: crate::transcript::Speaker::Tool { failed: false },
            body: "read_file src/lib.rs\nPath: src/lib.rs".into(),
        };
        let lines = super::render_entry(&entry, 40)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            ["⏺ read_file src/lib.rs", "  ⎿ Path: src/lib.rs", ""]
        );
    }
}
