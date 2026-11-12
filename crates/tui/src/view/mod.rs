pub(crate) mod text_layout;
pub(crate) mod theme;

use crate::app::{App, InputMode};
use crate::transcript::{recent_activity_lines, transcript_window_start};
use crate::view::text_layout::*;
use crate::view::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph},
};
use unicode_width::UnicodeWidthChar;

const SLASH_POPUP_MAX_ROWS: usize = 8;

impl App {
    pub(crate) fn draw(&self, frame: &mut Frame) {
        frame.render_widget(Block::new().style(Style::default().bg(BG)), frame.area());

        let vertical = Layout::vertical([
            Constraint::Length(10),
            Constraint::Min(8),
            Constraint::Length(8),
            Constraint::Length(2),
            Constraint::Length(1),
        ]);
        let [
            header_area,
            transcript_area,
            lower_area,
            composer_area,
            footer_area,
        ] = vertical.areas(frame.area());
        let [activity_area, command_output_area] =
            Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
                .areas(lower_area);

        self.draw_header(frame, header_area);
        self.draw_transcript(frame, transcript_area);
        self.draw_activity(frame, activity_area);
        self.draw_command_output(frame, command_output_area);
        self.draw_composer(frame, composer_area);
        self.draw_footer(frame, footer_area);
        self.draw_slash_popup(frame, composer_area);
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
                        Style::default().fg(LILAC).add_modifier(Modifier::BOLD),
                        Style::default().fg(TEXT),
                    )
                } else {
                    ("  ", Style::default().fg(BLUE), Style::default().fg(MUTED))
                };
                Line::from(vec![
                    Span::styled(marker, Style::default().fg(LILAC)),
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
            .title(" commands · ↑↓ select · Tab complete · Enter run · Esc close ")
            .border_style(Style::default().fg(CODE_BORDER))
            .style(Style::default().bg(BG));
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(Text::from(lines)).block(block), area);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let mut logo_lines = build_logo_rows(&LOGO_WORDMARK);
        logo_lines.push(Line::from(vec![
            Span::styled("model ", Style::default().fg(SOFT)),
            Span::styled(&self.model_label, Style::default().fg(BLUE)),
        ]));
        logo_lines.push(Line::from(vec![
            Span::styled("dir ", Style::default().fg(SOFT)),
            Span::styled(&self.workspace_label, Style::default().fg(LILAC)),
        ]));
        logo_lines.push(Line::from(vec![
            Span::styled("socium ", Style::default().fg(SOFT)),
            Span::styled(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(PINK),
            ),
        ]));
        frame.render_widget(
            Paragraph::new(Text::from(logo_lines)).style(Style::default().bg(BG)),
            area,
        );
    }

    fn draw_transcript(&self, frame: &mut Frame, area: Rect) {
        let transcript =
            Paragraph::new(self.visible_transcript(area.width as usize, area.height as usize))
                .style(Style::default().bg(BG).fg(TEXT));
        frame.render_widget(transcript, area);
    }

    fn draw_activity(&self, frame: &mut Frame, area: Rect) {
        let lines =
            recent_activity_lines(&self.activity_log, area.height.saturating_sub(2) as usize);
        let block = Block::bordered()
            .title(" activity ")
            .border_style(Style::default().fg(CODE_BORDER))
            .style(Style::default().bg(BG));
        let text = if lines.is_empty() {
            Text::from(vec![Line::from(Span::styled(
                "No tool activity yet",
                Style::default().fg(SOFT),
            ))])
        } else {
            Text::from(
                lines
                    .into_iter()
                    .map(|line| {
                        Line::from(vec![
                            Span::styled("• ", Style::default().fg(PINK)),
                            Span::styled(line, Style::default().fg(TEXT)),
                        ])
                    })
                    .collect::<Vec<_>>(),
            )
        };
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_command_output(&self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .title(" last command ")
            .border_style(Style::default().fg(CODE_BORDER))
            .style(Style::default().bg(BG));
        let text = if let Some(output) = &self.last_command_output {
            Text::from(
                command_output_preview_lines(
                    output,
                    area.width.saturating_sub(2) as usize,
                    area.height.saturating_sub(2) as usize,
                )
                .into_iter()
                .map(|line| Line::from(Span::styled(line, Style::default().fg(TEXT))))
                .collect::<Vec<_>>(),
            )
        } else {
            Text::from(vec![Line::from(Span::styled(
                "No command output yet",
                Style::default().fg(SOFT),
            ))])
        };
        frame.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_composer(&self, frame: &mut Frame, area: Rect) {
        let available_cells = area.width.saturating_sub(2) as usize;
        let (visible_input, visible_cursor) = self.visible_input_window(available_cells);

        let line = if self.input.is_empty() {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(MUTED)),
                Span::styled(
                    "Ask Socium to inspect files, explain code, or run tasks",
                    Style::default().fg(SOFT),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled("> ", Style::default().fg(MUTED)),
                Span::styled(visible_input, Style::default().fg(MUTED)),
            ])
        };

        frame.render_widget(Paragraph::new(line).style(Style::default().bg(BG)), area);

        if matches!(self.input_mode, InputMode::Editing) {
            frame.set_cursor_position(Position::new(area.x + 2 + visible_cursor as u16, area.y));
        }
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let [left, center, right] = Layout::horizontal([
            Constraint::Percentage(38),
            Constraint::Percentage(22),
            Constraint::Percentage(40),
        ])
        .areas(area);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(&self.workspace_label, Style::default().fg(BLUE)),
                Span::styled(" ", Style::default().fg(MUTED)),
                Span::styled(
                    format!(
                        "({})",
                        self.status.clone().unwrap_or_else(|| "idle".to_string())
                    ),
                    Style::default().fg(MUTED),
                ),
            ]))
            .style(Style::default().bg(BG)),
            left,
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                "host (ask)",
                Style::default().fg(PINK),
            )]))
            .style(Style::default().bg(BG)),
            center,
        );

        let latency_label = self
            .last_latency_ms
            .map(|latency| format!("{latency}ms"))
            .unwrap_or_else(|| "--".to_string());
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(&self.model_label, Style::default().fg(BLUE)),
                Span::styled(" ", Style::default().fg(MUTED)),
                Span::styled(
                    format!(
                        "({}% context left, {latency_label})",
                        self.context_metrics.context_left_percentage
                    ),
                    Style::default().fg(MUTED),
                ),
                Span::styled(" ", Style::default().fg(MUTED)),
                Span::styled(
                    // A trailing '*' marks a provider-measured token count;
                    // without it the value is a heuristic estimate.
                    match self.context_metrics.measured_input_tokens {
                        Some(measured) => {
                            format!("{measured}*/{}", self.context_metrics.input_budget_tokens)
                        }
                        None => format!(
                            "{}/{}",
                            self.context_metrics.estimated_input_tokens,
                            self.context_metrics.input_budget_tokens
                        ),
                    },
                    Style::default().fg(SOFT),
                ),
                Span::styled(" ", Style::default().fg(MUTED)),
                Span::styled(
                    if self.context_metrics.summary_active {
                        format!("summary {}", self.context_metrics.summarized_messages)
                    } else {
                        "summary off".to_string()
                    },
                    Style::default().fg(PINK),
                ),
            ]))
            .style(Style::default().bg(BG)),
            right,
        );
    }

    fn visible_input_window(&self, width: usize) -> (String, usize) {
        if width == 0 {
            return (String::new(), 0);
        }

        // Terminal cells, not chars: CJK and other wide characters take two
        // cells, and the cursor must land on the cell after the preceding text.
        let widths = self
            .input
            .chars()
            .map(|c| c.width().unwrap_or(0))
            .collect::<Vec<_>>();
        let cursor = self.character_index.min(widths.len());
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
        let visible = self
            .input
            .chars()
            .zip(&widths)
            .skip(start)
            .take_while(|(_, w)| {
                used += **w;
                used <= width
            })
            .map(|(c, _)| c)
            .collect::<String>();
        (visible, before_cursor)
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
                    Span::styled("/help", Style::default().fg(LILAC)),
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

    fn render(app: &App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
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
        assert!(rows[composer - 3].contains("› /clear     Clear the current conversation"));
        assert!(rows[composer - 2].contains("  /compact"));
        assert!(rows[composer - 1].starts_with('└'));
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

    #[test]
    fn no_popup_for_ordinary_input() {
        let mut app = App::for_test();
        app.input = "hello".into();
        assert!(!render(&app).iter().any(|row| row.contains(" commands ")));
    }
}
