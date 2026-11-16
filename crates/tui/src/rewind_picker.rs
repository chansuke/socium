//! The prompt picker shown by `/rewind`, in place of the conversation, and
//! going back to the chosen prompt.

use crate::app::App;
use crate::transcript::Speaker;
use crate::view::text_layout::truncate_text;
use crate::view::theme::*;
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyModifiers},
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};
use socium_core::{RewindPoint, Rewound};

/// Rows above and below the list inside the border: the description, a
/// blank line, a blank line, and the key help.
const CHROME_ROWS: u16 = 4;

pub(crate) struct RewindPicker {
    /// Newest first.
    pub(crate) points: Vec<RewindPoint>,
    pub(crate) selected: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RewindOutcome {
    Open,
    /// The `turn_id` of the chosen prompt.
    Chosen(usize),
    Cancelled,
}

impl RewindPicker {
    /// `points` oldest first, as the agent lists them.
    pub(crate) fn new(mut points: Vec<RewindPoint>) -> Self {
        points.reverse();
        Self {
            points,
            selected: 0,
        }
    }

    pub(crate) fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> RewindOutcome {
        let last = self.points.len().saturating_sub(1);
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::PageDown => self.selected = (self.selected + 10).min(last),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.selected = last,
            KeyCode::Enter => {
                if let Some(point) = self.points.get(self.selected) {
                    return RewindOutcome::Chosen(point.turn_id);
                }
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                return RewindOutcome::Cancelled;
            }
            KeyCode::Esc | KeyCode::Char('q') => return RewindOutcome::Cancelled,
            _ => {}
        }
        RewindOutcome::Open
    }

    /// The picker's contents inside its border: the prompts that fit in
    /// `rows`, one line each, scrolled to keep the selection in view.
    pub(crate) fn lines(&self, width: u16, rows: u16) -> Vec<Line<'static>> {
        let inner = width.saturating_sub(2) as usize;
        let visible = rows.saturating_sub(CHROME_ROWS).max(1) as usize;
        let start = self
            .selected
            .saturating_sub(visible - 1)
            .min(self.points.len().saturating_sub(visible));
        let mut lines = vec![
            Line::from(Span::styled(
                truncate_text(
                    " Go back to before a prompt and edit it. Newest first. Files are not restored; /undo reverts edits.",
                    inner,
                ),
                Style::default().fg(MUTED),
            )),
            Line::from(""),
        ];
        lines.extend(
            self.points
                .iter()
                .enumerate()
                .skip(start)
                .take(visible)
                .map(|(index, point)| {
                    let selected = index == self.selected;
                    let marker = if selected { " ❯ " } else { "   " };
                    let prompt = point
                        .prompt
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    let style = if selected {
                        Style::default().fg(LILAC).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(TEXT)
                    };
                    Line::from(vec![
                        Span::styled(marker, style),
                        Span::styled(
                            truncate_text(&prompt, inner.saturating_sub(marker.chars().count())),
                            style,
                        ),
                    ])
                }),
        );
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            truncate_text(" ↑↓ select · Enter go back · Esc close", inner),
            Style::default().fg(MUTED),
        )));
        lines
    }

    /// Draws the picker over `area`, in place of the conversation.
    pub(crate) fn draw(&self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .title(" Rewind ")
            .border_style(Style::default().fg(LILAC))
            .style(Style::default().bg(BG));
        let rows = area.height.saturating_sub(2);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(self.lines(area.width, rows)).block(block),
            area,
        );
    }
}

impl App {
    /// Lists this conversation's prompts to choose one to go back to.
    pub(crate) fn open_rewind_picker(&mut self) {
        let Some(agent) = self.agent.as_ref() else {
            self.status = Some("Request in progress".into());
            return;
        };
        let points = agent.rewind_points();
        if points.is_empty() {
            self.push_message(
                Speaker::System,
                "No prompt in this conversation to go back to",
            );
            return;
        }
        self.rewind_picker = Some(RewindPicker::new(points));
        self.status = Some("Rewind: ↑↓ select · Enter go back · Esc close".into());
    }

    pub(crate) fn handle_rewind_picker_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let Some(picker) = self.rewind_picker.as_mut() else {
            return;
        };
        match picker.handle_key(code, modifiers) {
            RewindOutcome::Open => {}
            RewindOutcome::Cancelled => {
                self.rewind_picker = None;
                self.status = Some("Conversation unchanged".into());
            }
            RewindOutcome::Chosen(turn_id) => {
                self.rewind_picker = None;
                self.rewind(turn_id);
            }
        }
    }

    fn rewind(&mut self, turn_id: usize) {
        let Some(agent) = self.agent.as_mut() else {
            self.status = Some("Request in progress".into());
            return;
        };
        match agent.rewind(turn_id) {
            Ok(rewound) => self.show_rewound(rewound),
            Err(err) => {
                self.push_message(Speaker::System, format!("Cannot rewind: {err}"));
                self.status = Some("Conversation unchanged".into());
            }
        }
    }

    /// Shows the shortened conversation and puts the prompt back in the
    /// input.
    pub(crate) fn show_rewound(&mut self, rewound: Rewound) {
        self.show_agent_conversation();
        self.input = rewound.prompt;
        self.character_index = self.input.chars().count();
        self.history_index = None;
        self.history_draft.clear();
        self.push_message(
            Speaker::System,
            "Went back to before this prompt; edit it and press Enter to send. Files were not changed; /undo reverts Socium's edits.",
        );
        if let Some(warning) = rewound.warning {
            self.push_message(Speaker::System, warning);
        }
        self.status = Some("Rewound".into());
        self.refresh_context_metrics();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(turn_id: usize, prompt: &str) -> RewindPoint {
        RewindPoint {
            turn_id,
            prompt: prompt.into(),
        }
    }

    fn picker(count: usize) -> RewindPicker {
        RewindPicker::new(
            (0..count)
                .map(|i| point(i * 2, &format!("prompt {i}")))
                .collect(),
        )
    }

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn keys_move_within_bounds_and_choose_or_cancel() {
        let mut picker = picker(3);
        let none = KeyModifiers::NONE;
        assert_eq!(picker.handle_key(KeyCode::Up, none), RewindOutcome::Open);
        assert_eq!(
            picker.handle_key(KeyCode::Enter, none),
            RewindOutcome::Chosen(4),
            "the newest prompt is listed first"
        );
        for _ in 0..5 {
            picker.handle_key(KeyCode::Down, none);
        }
        assert_eq!(
            picker.handle_key(KeyCode::Enter, none),
            RewindOutcome::Chosen(0)
        );
        picker.handle_key(KeyCode::Char('k'), none);
        assert_eq!(
            picker.handle_key(KeyCode::Enter, none),
            RewindOutcome::Chosen(2)
        );
        for code in [KeyCode::Esc, KeyCode::Char('q')] {
            assert_eq!(picker.handle_key(code, none), RewindOutcome::Cancelled);
        }
        assert_eq!(
            picker.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            RewindOutcome::Cancelled
        );
        assert_eq!(
            picker.handle_key(KeyCode::Char('d'), none),
            RewindOutcome::Open
        );
    }

    #[test]
    fn rows_show_one_line_per_prompt_cut_to_the_width() {
        let mut picker = RewindPicker::new(vec![
            point(0, "first line\nsecond line"),
            point(2, &"長い".repeat(40)),
        ]);
        let lines = text(&picker.lines(40, 10));
        assert_eq!(lines.len(), 6);
        assert!(lines[2].starts_with(" ❯ 長い"), "{lines:?}");
        assert_eq!(lines[3], "   first line second line");
        for line in picker.lines(40, 10) {
            assert!(line.width() <= 38, "{line}");
        }

        picker = RewindPicker::new((0..20).map(|i| point(i, &format!("p{i}"))).collect());
        picker.handle_key(KeyCode::End, KeyModifiers::NONE);
        let lines = text(&picker.lines(40, 8));
        assert_eq!(lines[5], " ❯ p0");
    }

    #[test]
    fn closing_the_picker_keeps_the_conversation() {
        let mut app = App::for_test();
        app.rewind_picker = Some(picker(2));
        app.handle_rewind_picker_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.rewind_picker.is_none());
        assert_eq!(app.status.as_deref(), Some("Conversation unchanged"));
    }

    #[test]
    fn rewinding_while_a_request_runs_is_refused() {
        let mut app = App::for_test();
        app.open_rewind_picker();
        assert!(app.rewind_picker.is_none());

        app.rewind_picker = Some(picker(2));
        app.handle_rewind_picker_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.rewind_picker.is_none());
        assert_eq!(app.status.as_deref(), Some("Request in progress"));
    }

    #[test]
    fn the_rewound_prompt_is_put_back_in_the_input_to_edit() {
        let mut app = App::for_test();
        app.show_rewound(Rewound {
            prompt: "fix the bug\nin parser.rs".into(),
            warning: None,
        });
        assert_eq!(app.input, "fix the bug\nin parser.rs");
        assert_eq!(app.character_index, app.input.chars().count());
        let note = &app.messages.last().unwrap().body;
        assert!(note.contains("Files were not changed"), "{note}");
        assert!(note.contains("/undo"), "{note}");
    }
}
