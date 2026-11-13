pub(crate) mod activity;
pub(crate) mod text_layout;
pub(crate) mod theme;

use crate::app::{App, InputMode};
use crate::transcript::transcript_window_start;
use crate::view::text_layout::*;
use crate::view::theme::*;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph},
};
use socium_core::activity::Phase;
use unicode_width::UnicodeWidthChar;

const SLASH_POPUP_MAX_ROWS: usize = 8;
/// Terminal width from which the Activity pane becomes a right-hand column.
const WIDE_LAYOUT_COLUMNS: u16 = 140;
/// Terminal height from which the logo header is shown.
const HEADER_MIN_ROWS: u16 = 40;

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
pub(crate) fn layout(area: Rect, activity_fullscreen: bool) -> Areas {
    let header_rows = if area.height >= HEADER_MIN_ROWS {
        10
    } else {
        0
    };
    let [header, body, composer, footer] = Layout::vertical([
        Constraint::Length(header_rows),
        Constraint::Min(1),
        Constraint::Length(2),
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

impl App {
    pub(crate) fn draw(&self, frame: &mut Frame) {
        frame.render_widget(Block::new().style(Style::default().bg(BG)), frame.area());

        let areas = layout(frame.area(), self.activity.fullscreen);

        self.draw_header(frame, areas.header);
        self.draw_transcript(frame, areas.transcript);
        self.draw_activity(frame, areas.activity);
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
            .title(title)
            .border_style(Style::default().fg(if focused { LILAC } else { CODE_BORDER }))
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
                Style::default().fg(SOFT),
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

    fn render_at(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
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
        let column = header.chars().position(|c| c == '┌').unwrap();
        assert!(column >= 90, "activity starts at column {column}");
        assert!(rows.iter().any(|row| row.contains("⚙ read_file")));
        assert!(rows.iter().any(|row| row.contains("> explain the view")));
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
        for row in render_at(&app, 110, 30) {
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
        assert!(!render(&app).iter().any(|row| row.contains(" commands ")));
    }
}
