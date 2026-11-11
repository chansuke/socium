use crate::transcript::{ChatEntry, CommandOutputState, Speaker};
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

pub(crate) fn help_text() -> &'static str {
    "/help    Show available commands\n/clear   Clear the current conversation\n/compact Summarize old conversation context to free tokens\n/exit    Quit Socium\nEsc / Ctrl+C   Cancel a running task\ny / n          Allow once / deny a pending host command"
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
        Speaker::Thinking => render_thinking_card(&entry.body, width),
        Speaker::System => render_wrapped_text(&entry.body, width, "! ", "  ", PINK, TEXT, None),
    }
}

fn render_thinking_card(text: &str, width: usize) -> Vec<Line<'static>> {
    let card_width = width.saturating_sub(4).max(32);
    let inner_width = card_width.saturating_sub(4);
    let wrapped = wrap_multiline(text, inner_width, inner_width);
    let mut lines = vec![Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled(
            format!("┌{}┐", "─".repeat(card_width.saturating_sub(2))),
            Style::default().fg(CODE_BORDER),
        ),
    ])];

    let title = "↪ SociumAgent";
    let subtitle = wrapped
        .first()
        .cloned()
        .unwrap_or_else(|| "Working on it".to_string());
    lines.push(card_line(&format!("{title} {subtitle}"), card_width));

    for line in wrapped.into_iter().skip(1) {
        lines.push(card_line(&line, card_width));
    }

    lines.push(Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled(
            format!("└{}┘", "─".repeat(card_width.saturating_sub(2))),
            Style::default().fg(CODE_BORDER),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(": ", Style::default().fg(MUTED)),
        Span::styled("Socium is thinking", Style::default().fg(LILAC)),
        Span::styled(" (esc to cancel)", Style::default().fg(MUTED)),
    ]));
    lines.push(Line::from(""));
    lines
}

fn card_line(content: &str, card_width: usize) -> Line<'static> {
    let inner_width = card_width.saturating_sub(4);
    let visible = content.chars().take(inner_width).collect::<String>();
    let padding = inner_width.saturating_sub(visible.chars().count());

    Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled("│ ", Style::default().fg(CODE_BORDER)),
        Span::styled(visible, Style::default().fg(CODE_TEXT)),
        Span::styled(
            format!("{} │", " ".repeat(padding)),
            Style::default().fg(CODE_BORDER),
        ),
    ])
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
        width.saturating_sub(prefix.chars().count()),
        width.saturating_sub(indent.chars().count()),
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
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut width = first_width.max(1);

    for word in paragraph.split_whitespace() {
        for segment in split_long_word(word, width.max(1)) {
            if current.is_empty() {
                current.push_str(&segment);
                continue;
            }

            let next_len = current.chars().count() + 1 + segment.chars().count();
            if next_len <= width {
                current.push(' ');
                current.push_str(&segment);
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(&segment);
                width = next_width.max(1);
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

fn split_long_word(word: &str, width: usize) -> Vec<String> {
    if word.chars().count() <= width {
        return vec![word.to_string()];
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in word.chars() {
        current.push(ch);
        if current.chars().count() == width {
            parts.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        parts.push(current);
    }

    parts
}

fn render_code_block(label: &str, code_lines: &[String], width: usize) -> Vec<Line<'static>> {
    let outer_width = width.saturating_sub(2).max(24);
    let inner_width = outer_width.saturating_sub(2);
    let title = format!(" {} ", if label.is_empty() { "snippet" } else { label });
    let title_chars = title.chars().count().min(inner_width);
    let title_text = title.chars().take(title_chars).collect::<String>();
    let fill = inner_width.saturating_sub(title_text.chars().count());

    let mut lines = vec![Line::from(vec![
        Span::styled("  ", Style::default().fg(MUTED)),
        Span::styled(
            format!("┌{}{}┐", title_text, "─".repeat(fill)),
            Style::default().fg(CODE_BORDER),
        ),
    ])];

    for raw_line in code_lines {
        let visible = raw_line
            .chars()
            .take(inner_width.saturating_sub(2))
            .collect::<String>();
        let padding = inner_width.saturating_sub(2 + visible.chars().count());
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

pub(crate) fn truncate_text(text: &str, max_chars: usize) -> String {
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
    use super::command_output_preview_lines;
    use crate::transcript::CommandOutputState;

    #[test]
    fn help_text_lists_supported_slash_commands() {
        let help = super::help_text();

        assert!(help.contains("/help"));
        assert!(help.contains("/clear"));
        assert!(help.contains("/exit"));
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
}
