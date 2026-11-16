pub(crate) mod activity;
pub(crate) mod cells;
pub(crate) mod text_layout;
pub(crate) mod theme;

use crate::app::{App, InputMode};
use crate::approval::ApprovalPrompt;
use crate::model_picker::{ModelPicker, model_label};
use crate::transcript::transcript_window_start;
use crate::view::text_layout::*;
use crate::view::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, Paragraph},
};
use socium_core::activity::Phase;
use socium_core::providers::TokenUsage;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SLASH_POPUP_MAX_ROWS: usize = 8;
const COMPOSER_ROWS: u16 = 2;
/// Lines of a multi-line message shown at once; the rest scroll.
const COMPOSER_MAX_LINES: usize = 8;
const APPROVAL_SUMMARY_MAX_ROWS: usize = 3;
const MODEL_PICKER_MAX_ROWS: usize = 8;
/// Terminal width from which the Activity pane becomes a right-hand column.
const WIDE_LAYOUT_COLUMNS: u16 = 140;
/// Terminal height from which the header is shown.
const HEADER_MIN_ROWS: u16 = 32;
/// The header card: the name and version, then the model and the
/// directory, inside a rounded border.
const HEADER_CARD_ROWS: u16 = 4;
/// What the sandbox always enforces, shown at the header's right.
const SANDBOX_FACTS: [(&str, &str); 2] = [("sandbox", "workspace only"), ("network", "off")];

pub(crate) struct Areas {
    pub(crate) header: Rect,
    pub(crate) transcript: Rect,
    pub(crate) activity: Rect,
    pub(crate) composer: Rect,
    pub(crate) footer: Rect,
}

/// Splits the screen. Wide terminals put Activity beside the transcript;
/// narrower ones stack it below, sized to the screen. Full screen gives the
/// Activity pane the transcript's space as well.
pub(crate) fn layout(area: Rect, activity_fullscreen: bool, composer_rows: u16) -> Areas {
    let header_rows = if area.height >= HEADER_MIN_ROWS {
        HEADER_CARD_ROWS + 1
    } else {
        0
    };
    let [header, body, composer, footer] = Layout::vertical([
        Constraint::Length(header_rows),
        Constraint::Min(1),
        Constraint::Length(composer_rows),
        Constraint::Length(1),
    ])
    .areas(area);
    let (transcript, activity) = if activity_fullscreen {
        (Rect::new(body.x, body.y, body.width, 0), body)
    } else if area.width >= WIDE_LAYOUT_COLUMNS {
        let [transcript, activity] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(body);
        (transcript, activity)
    } else {
        let activity_rows = (body.height * 35 / 100).clamp(6.min(body.height), 24);
        let [transcript, activity] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(activity_rows)]).areas(body);
        (transcript, activity)
    };
    Areas {
        header,
        transcript,
        activity,
        composer,
        footer,
    }
}

/// The approval picker's contents inside its border.
fn approval_lines(prompt: &ApprovalPrompt, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(4) as usize;
    let mut lines = wrap_multiline(&prompt.summary, inner, inner)
        .into_iter()
        .take(APPROVAL_SUMMARY_MAX_ROWS)
        .map(|row| Line::from(Span::styled(format!(" {row}"), Style::default().fg(TEXT))))
        .collect::<Vec<_>>();
    lines.push(Line::from(""));
    for (index, choice) in prompt.choices().into_iter().enumerate() {
        let selected = choice == prompt.choice;
        let (marker, style) = if selected {
            (
                "❯",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            )
        } else {
            (" ", Style::default().fg(MUTED))
        };
        lines.push(Line::from(Span::styled(
            format!(" {marker} {}. {}", index + 1, prompt.label(choice)),
            style,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓ select · Enter confirm · PgUp/PgDn/Home/End review · Esc cancel",
        Style::default().fg(MUTED),
    )));
    lines
}

/// The full approval request, scrolled from its first line. Spacing is kept
/// as sent, so diff indentation survives, and nothing is cut off: long
/// lines wrap by terminal cells.
fn draw_approval_details(frame: &mut Frame, area: Rect, prompt: &ApprovalPrompt) {
    let width = area.width.saturating_sub(2) as usize;
    let rows = area.height.saturating_sub(2) as usize;
    let lines = cells::wrap_preformatted(&prompt.details, width);
    let max_scroll = lines.len().saturating_sub(rows);
    let scroll = prompt.scroll.min(max_scroll);
    prompt
        .viewport
        .set(crate::approval::DetailsViewport { rows, max_scroll });
    let last = (scroll + rows).min(lines.len());
    let more = if last < lines.len() {
        " · more below"
    } else {
        ""
    };
    let title = format!(
        " Review {} · lines {}-{last} of {}{more} ",
        prompt.tool,
        (scroll + 1).min(last),
        lines.len()
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(cells::truncate_cells(&title, width))
        .border_style(Style::default().fg(FOCUS_BORDER))
        .style(Style::default().bg(BG));
    let visible = lines
        .into_iter()
        .skip(scroll)
        .take(rows)
        .map(|row| Line::from(Span::styled(row, Style::default().fg(TEXT))))
        .collect::<Vec<_>>();
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(visible).block(block), area);
}

/// The model picker's contents inside its border, laid out like Claude
/// Code's: a description, numbered models with a check on the current one,
/// then the key help.
fn model_picker_lines(picker: &ModelPicker, width: u16) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(4) as usize;
    let label = |index: usize, choice: &socium_core::ModelChoice| {
        let check = if choice.active { " ✔" } else { "" };
        format!("{}. {}{check}", index + 1, choice.alias)
    };
    let label_width = picker
        .choices
        .iter()
        .enumerate()
        .map(|(index, choice)| label(index, choice).width())
        .max()
        .unwrap_or(0);
    let start = picker
        .selected
        .saturating_sub(MODEL_PICKER_MAX_ROWS - 1)
        .min(picker.choices.len().saturating_sub(MODEL_PICKER_MAX_ROWS));
    let mut lines = vec![
        Line::from(Span::styled(
            truncate_text(
                " Switch the model for this conversation. The choice is saved with it.",
                inner,
            ),
            Style::default().fg(MUTED),
        )),
        Line::from(""),
    ];
    lines.extend(
        picker
            .choices
            .iter()
            .enumerate()
            .skip(start)
            .take(MODEL_PICKER_MAX_ROWS)
            .map(|(index, choice)| {
                let selected = index == picker.selected;
                let marker = if selected { "❯" } else { " " };
                let label = label(index, choice);
                let head = format!(
                    " {marker} {label}{}  ",
                    " ".repeat(label_width - label.width())
                );
                let detail = match (&choice.unavailable, &choice.description) {
                    (Some(reason), _) => reason.clone(),
                    (None, Some(description)) => {
                        format!("{description} · {}", model_label(choice))
                    }
                    (None, None) => model_label(choice),
                };
                let detail = truncate_text(&detail, inner.saturating_sub(head.width()));
                let (head_style, detail_style) = if choice.unavailable.is_some() {
                    (Style::default().fg(MUTED), Style::default().fg(MUTED))
                } else if selected {
                    (
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                        Style::default().fg(TEXT),
                    )
                } else {
                    (Style::default().fg(TEXT), Style::default().fg(MUTED))
                };
                Line::from(vec![
                    Span::styled(head, head_style),
                    Span::styled(detail, detail_style),
                ])
            }),
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Enter to confirm · 1-9 to choose · Esc to exit",
        Style::default().fg(MUTED),
    )));
    lines
}

impl App {
    pub(crate) fn draw(&self, frame: &mut Frame) {
        frame.render_widget(Block::new().style(Style::default().bg(BG)), frame.area());

        let width = frame.area().width;
        let composer_rows = if let Some(prompt) = &self.approval {
            approval_lines(prompt, width).len() as u16 + 2
        } else if let Some(picker) = &self.model_picker {
            model_picker_lines(picker, width).len() as u16 + 2
        } else {
            let lines = self.input.split('\n').count().min(COMPOSER_MAX_LINES);
            COMPOSER_ROWS + lines as u16 - 1
        };
        let areas = layout(frame.area(), self.activity.fullscreen, composer_rows);

        self.draw_header(frame, areas.header);
        if let Some(prompt) = &self.approval {
            // The request under review replaces the transcript and Activity
            // so that a long diff gets the whole body of the screen.
            draw_approval_details(frame, areas.transcript.union(areas.activity), prompt);
        } else if let Some(picker) = &self.session_picker {
            picker.draw(
                frame,
                areas.transcript.union(areas.activity),
                std::time::SystemTime::now(),
            );
        } else {
            self.draw_transcript(frame, areas.transcript);
            self.draw_activity(frame, areas.activity);
        }
        self.draw_composer(frame, areas.composer);
        self.draw_footer(frame, areas.footer);
        self.draw_slash_popup(frame, areas.composer);
    }

    /// Overlays command suggestions directly above the composer.
    fn draw_slash_popup(&self, frame: &mut Frame, composer_area: Rect) {
        let suggestions = self.slash_suggestions();
        if suggestions.is_empty() {
            return;
        }
        let max_rows = SLASH_POPUP_MAX_ROWS.min(composer_area.y.saturating_sub(2) as usize);
        if max_rows == 0 {
            return;
        }
        let selected = self.slash_selection.min(suggestions.len() - 1);
        let rows = suggestions.len().min(max_rows);
        let start = selected.saturating_sub(rows - 1);
        let name_width = suggestions
            .iter()
            .map(|command| command.name.len() + 1)
            .max()
            .unwrap_or(0);
        let lines = suggestions
            .iter()
            .enumerate()
            .skip(start)
            .take(rows)
            .map(|(index, command)| {
                let (marker, name_style, description_style) = if index == selected {
                    (
                        "› ",
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                        Style::default().fg(TEXT),
                    )
                } else {
                    ("  ", Style::default().fg(TEXT), Style::default().fg(MUTED))
                };
                Line::from(vec![
                    Span::styled(marker, Style::default().fg(ACCENT)),
                    Span::styled(
                        format!("{:<name_width$}  ", format!("/{}", command.name)),
                        name_style,
                    ),
                    Span::styled(command.description, description_style),
                ])
            })
            .collect::<Vec<_>>();
        let height = rows as u16 + 2;
        let area = Rect::new(
            composer_area.x,
            composer_area.y - height,
            composer_area.width,
            height,
        );
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(" commands · ↑↓ select · Tab complete · Enter run · Esc close ")
            .border_style(Style::default().fg(BORDER))
            .style(Style::default().bg(BG));
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
    }

    /// A rounded card with the name and version, then the model and the
    /// directory, sized to its contents; what the sandbox enforces sits at
    /// the right when it fits.
    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let lines = vec![
            Line::from(vec![
                Span::styled(
                    "›_ ",
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "socium",
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", env!("CARGO_PKG_VERSION")),
                    Style::default().fg(MUTED),
                ),
            ]),
            Line::from(vec![
                Span::styled(&self.model_label, Style::default().fg(TEXT)),
                Span::styled(" · ", Style::default().fg(MUTED)),
                Span::styled(&self.workspace_label, Style::default().fg(MUTED)),
            ]),
        ];
        // One cell of padding on each side inside the border.
        let content = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
        let card = Rect {
            x: area.x + 1,
            width: (content + 4).min(area.width.saturating_sub(1)),
            height: HEADER_CARD_ROWS.min(area.height),
            ..area
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(BORDER))
            .padding(ratatui::widgets::Padding::horizontal(1))
            .style(Style::default().bg(BG));
        frame.render_widget(Paragraph::new(lines).block(block), card);

        let facts = SANDBOX_FACTS
            .iter()
            .map(|(name, value)| {
                Line::from(vec![
                    Span::styled(format!("{name} "), Style::default().fg(MUTED)),
                    Span::styled(*value, Style::default().fg(TEXT)),
                ])
            })
            .collect::<Vec<_>>();
        let facts_width = facts.iter().map(Line::width).max().unwrap_or(0) as u16;
        let right = area.right().saturating_sub(facts_width + 2);
        if right > card.right() + 2 && card.height >= HEADER_CARD_ROWS {
            let facts_area = Rect::new(right, card.y + 1, facts_width, card.height - 2);
            frame.render_widget(
                Paragraph::new(facts)
                    .alignment(ratatui::layout::Alignment::Right)
                    .style(Style::default().bg(BG)),
                facts_area,
            );
        }
    }

    fn draw_transcript(&self, frame: &mut Frame, area: Rect) {
        let transcript =
            Paragraph::new(self.visible_transcript(area.width as usize, area.height as usize))
                .style(Style::default().bg(BG).fg(TEXT));
        frame.render_widget(transcript, area);
    }

    fn draw_activity(&self, frame: &mut Frame, area: Rect) {
        let focused = self.activity.focused;
        let raw_title;
        let title = if let Some(view) = &self.activity.raw_view {
            raw_title = format!(
                " raw stream · round {} · ↑↓ scroll · r / Esc back ",
                view.round
            );
            raw_title.as_str()
        } else {
            let phase = match self.activity.phase() {
                Phase::Idle => String::new(),
                phase => format!(" · {}", phase.label()),
            };
            raw_title = if focused {
                format!(" activity{phase} · ↑↓ select · Enter expand · r raw · z full · Esc back ")
            } else {
                format!(" activity{phase} · Ctrl+A focus ")
            };
            raw_title.as_str()
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(title)
            .border_style(Style::default().fg(if focused { FOCUS_BORDER } else { BORDER }))
            .style(Style::default().bg(BG));
        let text = if let Some(view) = &self.activity.raw_view {
            Text::from(activity::raw_lines(
                &self.activity,
                view,
                area.width.saturating_sub(2) as usize,
                area.height.saturating_sub(2) as usize,
            ))
        } else if self.activity.is_empty() {
            Text::from(vec![Line::from(Span::styled(
                "Thoughts, tool calls, and token use appear here",
                Style::default().fg(MUTED),
            ))])
        } else {
            Text::from(activity::activity_lines(
                &self.activity,
                area.width.saturating_sub(2) as usize,
                area.height.saturating_sub(2) as usize,
            ))
        };
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_composer(&self, frame: &mut Frame, area: Rect) {
        if let Some(prompt) = &self.approval {
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .title(format!(" Approve {} ", prompt.tool))
                .border_style(Style::default().fg(FOCUS_BORDER))
                .style(Style::default().bg(BG));
            frame.render_widget(
                Paragraph::new(approval_lines(prompt, area.width)).block(block),
                area,
            );
            return;
        }
        if let Some(picker) = &self.model_picker {
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    " Select model ",
                    Style::default()
                        .fg(FOCUS_BORDER)
                        .add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::default().fg(FOCUS_BORDER))
                .style(Style::default().bg(BG));
            frame.render_widget(
                Paragraph::new(model_picker_lines(picker, area.width)).block(block),
                area,
            );
            return;
        }
        let available_cells = area.width.saturating_sub(2) as usize;
        if self.input.contains('\n') {
            self.draw_multiline_composer(frame, area, available_cells);
            return;
        }
        let (visible_input, visible_cursor, visible_start) =
            self.visible_input_window(available_cells);

        let line = if self.input.is_empty() {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(MUTED)),
                Span::styled(
                    "Ask Socium to inspect files, explain code, or run tasks",
                    Style::default().fg(MUTED),
                ),
            ])
        } else {
            // A fully typed command name stands out, as in Claude Code.
            let highlighted = crate::slash_command::typed_command_len(&self.input)
                .unwrap_or(0)
                .saturating_sub(visible_start);
            let split = visible_input
                .char_indices()
                .nth(highlighted)
                .map_or(visible_input.len(), |(index, _)| index);
            let (command, rest) = visible_input.split_at(split);
            Line::from(vec![
                Span::styled("> ", Style::default().fg(MUTED)),
                Span::styled(
                    command.to_string(),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(rest.to_string(), Style::default().fg(MUTED)),
            ])
        };

        frame.render_widget(Paragraph::new(line).style(Style::default().bg(BG)), area);

        if matches!(self.input_mode, InputMode::Editing) {
            frame.set_cursor_position(Position::new(area.x + 2 + visible_cursor as u16, area.y));
        }
    }

    /// A message of several lines: the lines around the cursor, each
    /// scrolled sideways on its own, with a count of hidden ones.
    fn draw_multiline_composer(&self, frame: &mut Frame, area: Rect, width: usize) {
        let lines = self.input.split('\n').collect::<Vec<_>>();
        let before = self
            .input
            .chars()
            .take(self.character_index)
            .collect::<String>();
        let cursor_row = before.matches('\n').count();
        let cursor_column = before
            .rsplit('\n')
            .next()
            .map_or(0, |line| line.chars().count());
        let first = (cursor_row + 1).saturating_sub(COMPOSER_MAX_LINES);
        let mut rows = Vec::new();
        let mut cursor_cell = 0;
        for (index, line) in lines
            .iter()
            .enumerate()
            .skip(first)
            .take(COMPOSER_MAX_LINES)
        {
            let column = if index == cursor_row {
                cursor_column
            } else {
                0
            };
            let (visible, cell, _) = visible_window(line, column, width);
            if index == cursor_row {
                cursor_cell = cell;
            }
            let prefix = if index == 0 { "> " } else { "  " };
            rows.push(Line::from(vec![
                Span::styled(prefix, Style::default().fg(MUTED)),
                Span::styled(visible, Style::default().fg(MUTED)),
            ]));
        }
        let hidden = lines.len() - rows.len();
        if hidden > 0 {
            rows.push(Line::from(Span::styled(
                format!("  ({hidden} more lines)"),
                Style::default().fg(MUTED),
            )));
        }
        frame.render_widget(
            Paragraph::new(Text::from(rows)).style(Style::default().bg(BG)),
            area,
        );
        if matches!(self.input_mode, InputMode::Editing) {
            frame.set_cursor_position(Position::new(
                area.x + 2 + cursor_cell as u16,
                area.y + (cursor_row - first) as u16,
            ));
        }
    }

    /// What Socium is doing and how commands are approved at the left;
    /// context, latency, and usage at the right.
    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let muted = |text: String| Span::styled(text, Style::default().fg(MUTED));
        let approval = if self.commands_run_without_asking {
            "commands run without asking"
        } else {
            "ask before commands"
        };
        let left = Line::from(vec![
            Span::styled(
                self.status.clone().unwrap_or_else(|| "idle".to_string()),
                Style::default().fg(TEXT),
            ),
            muted(format!(" · {approval}")),
        ]);

        let latency = self
            .last_latency_ms
            .map(|latency| format!("{latency}ms"))
            .unwrap_or_else(|| "--".to_string());
        // A trailing '*' marks a provider-measured token count; without it
        // the value is a heuristic estimate.
        let tokens = match self.context_metrics.measured_input_tokens {
            Some(measured) => format!("{measured}*/{}", self.context_metrics.input_budget_tokens),
            None => format!(
                "{}/{}",
                self.context_metrics.estimated_input_tokens,
                self.context_metrics.input_budget_tokens
            ),
        };
        let mut right = format!(
            "{}% context left ({tokens}) · {latency} · {}",
            self.context_metrics.context_left_percentage,
            format_session_usage(&self.session_usage.0, self.session_usage.1)
        );
        if self.context_metrics.summary_active {
            right.push_str(&format!(
                " · summary {}",
                self.context_metrics.summarized_messages
            ));
        }
        let right = Line::from(muted(right));

        // The right side keeps its full width; the status gives way.
        let right_width = (right.width() as u16 + 1).min(area.width);
        let [left_area, right_area] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(area);
        let padded = Rect {
            x: left_area.x + 1,
            width: left_area.width.saturating_sub(2),
            ..left_area
        };
        frame.render_widget(Paragraph::new(left).style(Style::default().bg(BG)), padded);
        frame.render_widget(
            Paragraph::new(right)
                .alignment(ratatui::layout::Alignment::Right)
                .style(Style::default().bg(BG)),
            Rect {
                width: right_area.width.saturating_sub(1),
                ..right_area
            },
        );
    }

    /// The visible slice of the input, the cursor's cell within it, and the
    /// index of its first char in the whole input.
    fn visible_input_window(&self, width: usize) -> (String, usize, usize) {
        visible_window(&self.input, self.character_index, width)
    }

    fn visible_transcript(&self, width: usize, height: usize) -> Text<'static> {
        if self.messages.is_empty() {
            return Text::from(vec![
                Line::from(Span::styled(
                    "Tips for getting started:",
                    Style::default().fg(TEXT),
                )),
                Line::from(Span::styled(
                    "1. Ask questions, edit files, or run commands.",
                    Style::default().fg(TEXT),
                )),
                Line::from(Span::styled(
                    "2. Be specific for the best results.",
                    Style::default().fg(TEXT),
                )),
                Line::from(vec![
                    Span::styled("3. ", Style::default().fg(TEXT)),
                    Span::styled("/help", Style::default().fg(ACCENT)),
                    Span::styled(" for more information.", Style::default().fg(TEXT)),
                ]),
            ]);
        }

        let mut lines = Vec::new();
        for entry in &self.messages {
            lines.extend(render_entry(entry, width.max(24)));
        }

        let start = transcript_window_start(lines.len(), height.max(1), self.transcript_scroll);
        Text::from(lines.into_iter().skip(start).collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use ratatui::{Terminal, backend::TestBackend};
    use unicode_width::UnicodeWidthStr;

    fn render(app: &App) -> Vec<String> {
        render_at(app, 80, 30)
    }

    fn render_at(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        // A wide grapheme fills its cell and the next; read it once.
        (0..buffer.area.height)
            .map(|y| {
                let mut row = String::new();
                let mut x = 0;
                while x < buffer.area.width {
                    let symbol = buffer[(x, y)].symbol();
                    row.push_str(symbol);
                    x += symbol.width().max(1) as u16;
                }
                row
            })
            .collect()
    }

    #[test]
    fn a_message_of_several_lines_shows_each_line() {
        let mut app = App::for_test();
        app.insert_text("first line\nsecond line");
        let rows = render(&app);
        let first = rows.iter().position(|row| row.starts_with("> first line"));
        let first = first.expect("first line shown");
        assert!(rows[first + 1].starts_with("  second line"), "{rows:#?}");
    }

    /// A commit review as the core sends it: status, then a long diff.
    fn long_commit_review() -> String {
        let mut diff = String::from(
            "Approve sandboxed command: git_commit\nRepository state:\n# branch.head main\nRequested diff:\ndiff --git a/src/lib.rs b/src/lib.rs\n",
        );
        for line in 1..=120 {
            diff.push_str(&format!("+\tfn step_{line}() {{ /* 変更 {line} */ }}\n"));
        }
        diff.push_str("-END OF DIFF");
        diff
    }

    fn details_rows(rows: &[String]) -> Vec<String> {
        let top = rows
            .iter()
            .position(|row| row.starts_with("╭ Review git_commit"))
            .unwrap();
        rows[top + 1..]
            .iter()
            .take_while(|row| row.starts_with('│'))
            .map(|row| {
                row.trim_start_matches('│')
                    .trim_end_matches('│')
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn a_long_diff_can_be_read_from_first_to_last_line_at_80x24() {
        use ratatui::crossterm::event::KeyCode;
        let mut app = App::for_test();
        app.open_approval(
            3,
            "git_commit".into(),
            "fix parser".into(),
            long_commit_review(),
            None,
        );

        let rows = render_at(&app, 80, 24);
        assert_eq!(rows.len(), 24);
        assert!(
            rows.iter()
                .any(|row| row.starts_with("╭ Approve git_commit "))
        );
        let first = details_rows(&rows);
        assert!(first.len() >= 10, "{rows:#?}");
        assert_eq!(first[0], "Approve sandboxed command: git_commit");
        assert!(
            rows.iter()
                .any(|row| row.contains("lines 1-") && row.contains("of 126"))
        );

        // Page through the whole request; every diff line appears once.
        let mut seen = first.clone();
        for _ in 0..40 {
            app.handle_approval_key(KeyCode::PageDown);
            let page = details_rows(&render_at(&app, 80, 24));
            seen.extend(page);
        }
        for line in 1..=120 {
            let expected = format!("+   fn step_{line}() {{ /* 変更 {line} */ }}");
            assert!(seen.contains(&expected), "missing {expected}");
        }
        assert!(seen.iter().any(|row| row == "-END OF DIFF"));
        assert!(app.approval.is_some(), "paging never answers");

        app.handle_approval_key(KeyCode::Home);
        assert_eq!(details_rows(&render_at(&app, 80, 24))[0], first[0]);
        app.handle_approval_key(KeyCode::End);
        assert_eq!(
            details_rows(&render_at(&app, 80, 24)).last().unwrap(),
            "-END OF DIFF"
        );
    }

    #[test]
    fn approval_picker_replaces_the_composer_and_marks_the_choice() {
        let mut app = App::for_test();
        app.open_approval(
            1,
            "run_command".into(),
            "cargo test".into(),
            "Approve sandboxed command: run_command".into(),
            None,
        );
        let rows = render(&app);
        let top = rows
            .iter()
            .position(|row| row.starts_with("╭ Approve run_command "))
            .unwrap();

        assert!(rows[top + 1].starts_with("│ cargo test "));
        assert!(rows[top + 3].starts_with("│ ❯ 1. Allow once"));
        assert!(rows[top + 4].starts_with("│   2. Deny"));
        assert!(rows[top + 6].contains("↑↓ select · Enter confirm · PgUp/PgDn/Home/End review"));
        assert!(rows[top + 7].starts_with('╰'));
        assert_eq!(top + 8, rows.len() - 1, "the footer stays on the last row");
        assert!(!rows.iter().any(|row| row.starts_with("> ")));

        app.handle_approval_key(ratatui::crossterm::event::KeyCode::Down);
        let rows = render(&app);
        assert!(rows[top + 3].starts_with("│   1. Allow once"));
        assert!(rows[top + 4].starts_with("│ ❯ 2. Deny"));
    }

    #[test]
    fn the_session_picker_replaces_the_conversation() {
        let mut app = App::for_test();
        app.push_message(crate::transcript::Speaker::User, "hidden while choosing");
        app.session_picker = Some(crate::session_picker::SessionPicker::new(vec![
            socium_core::SessionSummary {
                id: "20261114-000000-0000".into(),
                updated: std::time::SystemTime::now(),
                message_count: 4,
                preview: "fix the README".into(),
                active_model: None,
                name: Some("readme".into()),
                readable: true,
            },
        ]));
        let rows = render(&app);
        let top = rows
            .iter()
            .position(|row| row.starts_with("╭ Resume · 20261114-000000-0000 "))
            .unwrap();
        assert!(rows[top + 3].contains("❯ just now     4 msgs  readme · fix the README"));
        assert!(!rows.iter().any(|row| row.contains("hidden while choosing")));
    }

    #[test]
    fn model_picker_lists_numbered_models_like_claude_code() {
        let mut app = App::for_test();
        let choice = |alias: &str, description: Option<&str>, active, unavailable: Option<&str>| {
            socium_core::ModelChoice {
                alias: alias.into(),
                provider: "deepseek".into(),
                model_id: format!("{alias}-id"),
                description: description.map(str::to_string),
                active,
                unavailable: unavailable.map(str::to_string),
            }
        };
        app.open_model_picker(vec![
            choice("opus", Some("Opus for complex work"), true, None),
            choice("ds-flash", None, false, None),
            choice("glm", None, false, Some("ZAI_API_KEY is not set")),
        ]);
        let rows = render(&app);
        let top = rows
            .iter()
            .position(|row| row.starts_with("╭ Select model "))
            .unwrap();

        assert!(rows[top + 1].starts_with("│ Switch the model for this conversation."));
        assert!(
            rows[top + 3].starts_with("│ ❯ 1. opus ✔    Opus for complex work · deepseek/opus-id")
        );
        assert!(rows[top + 4].starts_with("│   2. ds-flash  deepseek/ds-flash-id"));
        assert!(rows[top + 5].starts_with("│   3. glm       ZAI_API_KEY is not set"));
        assert!(rows[top + 7].contains("Enter to confirm · 1-9 to choose · Esc to exit"));
        assert!(rows[top + 8].starts_with('╰'));
        assert_eq!(top + 9, rows.len() - 1, "the footer stays on the last row");
    }

    #[test]
    fn a_fully_typed_command_is_highlighted_in_the_composer() {
        use crate::view::theme::{ACCENT, MUTED};

        let colors = |input: &str| {
            let mut app = App::for_test();
            app.input = input.into();
            app.character_index = input.chars().count();
            let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let buffer = terminal.backend().buffer();
            let y = (0..buffer.area.height)
                .find(|&y| buffer[(0, y)].symbol() == ">" && buffer[(2, y)].symbol() == "/")
                .unwrap();
            (2..2 + input.chars().count() as u16)
                .map(|x| buffer[(x, y)].fg)
                .collect::<Vec<_>>()
        };

        let typed = colors("/model glm");
        assert!(typed[..6].iter().all(|&fg| fg == ACCENT), "{typed:?}");
        assert!(typed[6..].iter().all(|&fg| fg == MUTED), "{typed:?}");
        assert!(colors("/mod").iter().all(|&fg| fg == MUTED));
        assert!(colors("/clear now").iter().all(|&fg| fg == MUTED));
    }

    #[test]
    fn slash_popup_sits_above_the_composer_with_selection_marker() {
        let mut app = App::for_test();
        app.input = "/c".into();
        app.character_index = 2;
        app.slash_selection = 1;
        let rows = render(&app);
        let composer = rows.iter().position(|row| row.starts_with("> /c")).unwrap();

        assert!(rows[composer - 5].contains(" commands "));
        assert!(rows[composer - 4].contains("  /commands  List available slash commands"));
        assert!(rows[composer - 3].contains("› /clear     Start a new conversation"));
        assert!(rows[composer - 2].contains("  /compact"));
        assert!(rows[composer - 1].starts_with('╰'));
    }

    fn composer_cursor(app: &App) -> (u16, String) {
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let cursor = terminal.get_cursor_position().unwrap();
        let buffer = terminal.backend().buffer();
        let row = (0..buffer.area.width)
            .map(|x| buffer[(x, cursor.y)].symbol())
            .collect::<String>();
        (cursor.x, row)
    }

    #[test]
    fn cursor_follows_display_width_of_wide_characters() {
        let mut app = App::for_test();
        app.input = "このプロジェクト".into();
        app.character_index = 8;
        let (x, row) = composer_cursor(&app);
        assert!(row.replace(' ', "").starts_with(">このプロジェクト"));
        assert_eq!(x, 2 + 16);

        app.character_index = 2;
        assert_eq!(composer_cursor(&app).0, 2 + 4);
    }

    #[test]
    fn long_wide_input_scrolls_to_keep_cursor_visible() {
        let mut app = App::for_test();
        app.input = "あ".repeat(60);
        app.character_index = 60;
        let (x, row) = composer_cursor(&app);
        // 78 cells wide; 38 wide chars (76 cells) plus the cursor cell fit.
        assert_eq!(x, 2 + 76);
        assert!(row.trim_end().ends_with('あ'));
    }

    fn app_with_activity() -> App {
        let mut app = App::for_test();
        app.push_message(crate::transcript::Speaker::User, "explain the view");
        for event in crate::activity::tests::sample_turn() {
            app.activity.apply(event);
        }
        app
    }

    #[test]
    fn small_terminal_keeps_input_transcript_and_activity_visible() {
        let rows = render_at(&app_with_activity(), 80, 24);

        // Footer on the last row, the two-row composer above it.
        assert!(rows[21].starts_with("> "), "composer row: {:?}", rows[21]);
        assert!(rows.iter().any(|row| row.contains("> explain the view")));
        let top = rows
            .iter()
            .position(|row| row.contains(" activity "))
            .unwrap();
        assert!(top < 21);
        assert!(
            rows[top..21]
                .iter()
                .any(|row| row.contains("response · 312 chars"))
        );
    }

    #[test]
    fn wide_terminal_places_activity_in_a_right_column() {
        let rows = render_at(&app_with_activity(), 160, 40);

        let header = rows.iter().find(|row| row.contains(" activity ")).unwrap();
        let column = header.chars().position(|c| c == '╭').unwrap();
        assert!(column >= 90, "activity starts at column {column}");
        assert!(rows.iter().any(|row| row.contains("⚙ read_file")));
        assert!(rows.iter().any(|row| row.contains("> explain the view")));
    }

    #[test]
    fn tall_terminal_shows_name_model_directory_and_sandbox_in_the_header() {
        let mut app = App::for_test();
        app.model_label = "claude-opus-5-5".into();
        app.workspace_label = "~/code/socium".into();
        let rows = render_at(&app, 80, 40);
        let version = env!("CARGO_PKG_VERSION");
        let card = |inner: &str| format!(" │ {inner:<31} │");

        assert_eq!(rows[0].trim_end(), format!(" ╭{}╮", "─".repeat(33)));
        assert!(rows[1].starts_with(&card(&format!("›_ socium {version}"))));
        assert!(rows[2].starts_with(&card("claude-opus-5-5 · ~/code/socium")));
        assert_eq!(rows[3].trim_end(), format!(" ╰{}╯", "─".repeat(33)));
        assert!(rows[4].trim().is_empty());
        // The sandbox's fixed limits, right-aligned beside the card.
        assert!(
            rows[1].trim_end().ends_with("sandbox workspace only"),
            "{:?}",
            rows[1]
        );
        assert!(rows[2].trim_end().ends_with("network off"), "{:?}", rows[2]);
        assert!(!render_at(&app, 80, 31)[0].contains("socium"));
    }

    #[test]
    fn footer_shows_status_and_how_commands_are_approved() {
        let mut app = App::for_test();
        let footer = |app: &App| render_at(app, 120, 30).pop().unwrap();
        assert!(
            footer(&app).starts_with(" idle · ask before commands"),
            "{:?}",
            footer(&app)
        );
        assert!(footer(&app).trim_end().ends_with("in 0 out 0"));

        app.commands_run_without_asking = true;
        app.status = Some("running cargo test".into());
        assert!(footer(&app).starts_with(" running cargo test · commands run without asking"));
    }

    #[test]
    fn focusing_activity_draws_its_frame_in_the_focus_color() {
        use crate::view::theme::{BORDER, FOCUS_BORDER};

        let corner_color = |focused| {
            let mut app = app_with_activity();
            app.activity.focused = focused;
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let buffer = terminal.backend().buffer();
            let y = (0..buffer.area.height)
                .find(|&y| buffer[(0, y)].symbol() == "╭")
                .unwrap();
            buffer[(0, y)].fg
        };

        assert_eq!(corner_color(true), FOCUS_BORDER);
        assert_eq!(corner_color(false), BORDER);
    }

    #[test]
    fn focused_full_screen_activity_marks_the_selection() {
        let mut app = app_with_activity();
        app.activity.focused = true;
        app.activity.fullscreen = true;
        app.activity.select_first();
        let rows = render_at(&app, 80, 24);

        assert!(rows[0].contains("Esc back"));
        assert!(rows[1].contains("› ━ turn 1"));
        assert!(!rows.iter().any(|row| row.contains("> explain the view")));
    }

    #[test]
    #[ignore = "prints a frame for manual inspection"]
    fn print_activity_frame() {
        let mut app = app_with_activity();
        app.activity.focused = true;
        app.activity.selected = Some(3);
        let width = std::env::var("FRAME_WIDTH").map_or(110, |w| w.parse().unwrap());
        let height = std::env::var("FRAME_HEIGHT").map_or(30, |h| h.parse().unwrap());
        for row in render_at(&app, width, height) {
            println!("{row}");
        }
    }

    #[test]
    fn title_shows_the_phase_and_signals_are_listed() {
        use socium_core::ActivityKind;
        let mut app = App::for_test();
        let event = |kind| socium_core::ActivityEvent {
            turn_id: 1,
            round: 1,
            at_ms: 0,
            agent: None,
            kind,
        };
        app.activity.apply(event(ActivityKind::TurnStarted));
        app.activity
            .apply(event(ActivityKind::RoundStarted { model: "m".into() }));
        for _ in 0..3 {
            app.activity.apply(event(ActivityKind::ToolStarted {
                call_id: "c".into(),
                name: "search_text".into(),
                input_summary: "push_activity".into(),
            }));
        }
        let rows = render_at(&app, 100, 30);

        assert!(
            rows.iter()
                .any(|row| row.contains(" activity · investigating · Ctrl+A focus "))
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("◇ Repeating: search_text push_activity started 3 times"))
        );
    }

    #[test]
    fn layouts_survive_tiny_terminals() {
        for (width, height) in [(1, 1), (20, 5), (80, 8), (200, 3)] {
            let mut app = app_with_activity();
            render_at(&app, width, height);
            app.activity.fullscreen = true;
            render_at(&app, width, height);
        }
    }

    #[test]
    fn no_popup_for_ordinary_input() {
        let mut app = App::for_test();
        app.input = "hello".into();
        assert!(!render(&app).iter().any(|row| row.contains(" commands · ")));
    }
}

/// Tokens sent and received so far, the share of input read from the
/// cache, and the cost when every model used has a price.
pub(crate) fn format_session_usage(usage: &TokenUsage, cost: Option<f64>) -> String {
    let compact = |tokens: u32| match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{:.1}k", tokens as f64 / 1_000.0),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    };
    let input = usage.total_input_tokens();
    let mut text = format!("in {} out {}", compact(input), compact(usage.output_tokens));
    if input > 0 && usage.cache_read_input_tokens > 0 {
        text.push_str(&format!(
            " ({}% cached)",
            u64::from(usage.cache_read_input_tokens) * 100 / u64::from(input)
        ));
    }
    if let Some(cost) = cost {
        text.push_str(&format!(" ${cost:.2}"));
    }
    text
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn session_usage_is_short_and_shows_cache_share_and_cost() {
        let usage = TokenUsage {
            input_tokens: 2_000,
            output_tokens: 1_234,
            cache_read_input_tokens: 8_000,
            ..TokenUsage::default()
        };
        assert_eq!(
            format_session_usage(&usage, Some(0.4167)),
            "in 10.0k out 1.2k (80% cached) $0.42"
        );
        assert_eq!(
            format_session_usage(&TokenUsage::default(), None),
            "in 0 out 0"
        );
    }
}

/// The slice of `text` that fits in `width` cells with the cursor (a char
/// index) visible, the cursor's cell within it, and the index of its
/// first char.
fn visible_window(text: &str, cursor: usize, width: usize) -> (String, usize, usize) {
    if width == 0 {
        return (String::new(), 0, 0);
    }

    // Terminal cells, not chars: CJK and other wide characters take two
    // cells, and the cursor must land on the cell after the preceding text.
    let widths = text
        .chars()
        .map(|c| c.width().unwrap_or(0))
        .collect::<Vec<_>>();
    let cursor = cursor.min(widths.len());
    let cursor_cell = |end: usize| widths.get(end).copied().unwrap_or(1).max(1);

    // Scroll just far enough that the text before the cursor plus the
    // cursor cell itself fits in the window.
    let mut start = 0;
    let mut before_cursor = widths[..cursor].iter().sum::<usize>();
    while start < cursor && before_cursor + cursor_cell(cursor) > width {
        before_cursor -= widths[start];
        start += 1;
    }

    let mut used = 0;
    let visible = text
        .chars()
        .zip(&widths)
        .skip(start)
        .take_while(|(_, w)| {
            used += **w;
            used <= width
        })
        .map(|(c, _)| c)
        .collect::<String>();
    (visible, before_cursor, start)
}
