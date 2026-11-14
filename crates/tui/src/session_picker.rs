//! The `-r` picker: chooses a saved session before the agent starts.

use crate::view::text_layout::truncate_text;
use crate::view::theme::*;
use color_eyre::Result;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use socium_core::SessionSummary;
use std::time::SystemTime;
use unicode_width::UnicodeWidthStr;

/// Rows above and below the list inside the border: the description, a
/// blank line, a blank line, and the key help.
const CHROME_ROWS: u16 = 4;

pub(crate) struct SessionPicker {
    pub(crate) sessions: Vec<SessionSummary>,
    pub(crate) selected: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PickerOutcome {
    Open,
    Chosen(String),
    Cancelled,
}

/// How long ago `then` was, in the largest whole unit.
pub(crate) fn age(then: SystemTime, now: SystemTime) -> String {
    let seconds = now.duration_since(then).unwrap_or_default().as_secs();
    match seconds {
        0..60 => "just now".to_string(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

impl SessionPicker {
    pub(crate) fn new(sessions: Vec<SessionSummary>) -> Self {
        Self {
            sessions,
            selected: 0,
        }
    }

    pub(crate) fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> PickerOutcome {
        let last = self.sessions.len().saturating_sub(1);
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::PageDown => self.selected = (self.selected + 10).min(last),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.selected = last,
            KeyCode::Enter => {
                if let Some(session) = self.sessions.get(self.selected) {
                    return PickerOutcome::Chosen(session.id.clone());
                }
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                return PickerOutcome::Cancelled;
            }
            KeyCode::Esc | KeyCode::Char('q') => return PickerOutcome::Cancelled,
            _ => {}
        }
        PickerOutcome::Open
    }

    /// The picker's contents inside its border: the sessions that fit in
    /// `rows`, scrolled to keep the selection in view.
    pub(crate) fn lines(&self, width: u16, rows: u16, now: SystemTime) -> Vec<Line<'static>> {
        let inner = width.saturating_sub(2) as usize;
        let visible = rows.saturating_sub(CHROME_ROWS).max(1) as usize;
        let start = self
            .selected
            .saturating_sub(visible - 1)
            .min(self.sessions.len().saturating_sub(visible));
        let ages = self
            .sessions
            .iter()
            .map(|session| age(session.updated, now))
            .collect::<Vec<_>>();
        let age_width = ages.iter().map(|age| age.width()).max().unwrap_or(0);
        let mut lines = vec![
            Line::from(Span::styled(
                truncate_text(
                    " Resume a conversation saved in this workspace. Newest first.",
                    inner,
                ),
                Style::default().fg(MUTED),
            )),
            Line::from(""),
        ];
        lines.extend(
            self.sessions
                .iter()
                .zip(&ages)
                .enumerate()
                .skip(start)
                .take(visible)
                .map(|(index, (session, age))| {
                    let selected = index == self.selected;
                    let marker = if selected { "❯" } else { " " };
                    let head = format!(
                        " {marker} {age}{}  {:>4} msgs  ",
                        " ".repeat(age_width - age.width()),
                        session.message_count
                    );
                    let preview = if session.preview.is_empty() {
                        format!("({})", session.id)
                    } else {
                        session.preview.clone()
                    };
                    let preview = truncate_text(&preview, inner.saturating_sub(head.width()));
                    let (head_style, preview_style) = if !session.readable {
                        (Style::default().fg(SOFT), Style::default().fg(SOFT))
                    } else if selected {
                        (
                            Style::default().fg(LILAC).add_modifier(Modifier::BOLD),
                            Style::default().fg(TEXT),
                        )
                    } else {
                        (Style::default().fg(MUTED), Style::default().fg(TEXT))
                    };
                    Line::from(vec![
                        Span::styled(head, head_style),
                        Span::styled(preview, preview_style),
                    ])
                }),
        );
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            truncate_text(" ↑↓ select · Enter resume · Esc quit", inner),
            Style::default().fg(MUTED),
        )));
        lines
    }

    fn draw(&self, frame: &mut Frame, now: SystemTime) {
        let area = frame.area();
        let title = self
            .sessions
            .get(self.selected)
            .map(|session| format!(" Resume · {} ", session.id))
            .unwrap_or_else(|| " Resume ".to_string());
        let block = Block::bordered()
            .title(title)
            .border_style(Style::default().fg(LILAC))
            .style(Style::default().bg(BG));
        let rows = area.height.saturating_sub(2);
        frame.render_widget(
            Paragraph::new(self.lines(area.width, rows, now)).block(block),
            area,
        );
    }

    /// Shows the picker until a session is chosen (its ID) or it is closed.
    pub(crate) fn run(mut self, terminal: &mut DefaultTerminal) -> Result<Option<String>> {
        loop {
            let now = SystemTime::now();
            terminal.draw(|frame| self.draw(frame, now))?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match self.handle_key(key.code, key.modifiers) {
                    PickerOutcome::Open => {}
                    PickerOutcome::Chosen(id) => return Ok(Some(id)),
                    PickerOutcome::Cancelled => return Ok(None),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn session(id: &str, seconds_ago: u64, preview: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            updated: SystemTime::UNIX_EPOCH + Duration::from_secs(100_000 - seconds_ago),
            message_count: 3,
            preview: preview.into(),
            active_model: None,
            readable: true,
        }
    }

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(100_000)
    }

    fn picker(count: usize) -> SessionPicker {
        SessionPicker::new(
            (0..count)
                .map(|i| {
                    session(
                        &format!("20261114-0000{i:02}-0000"),
                        i as u64 * 60,
                        &format!("prompt {i}"),
                    )
                })
                .collect(),
        )
    }

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn ages_use_the_largest_whole_unit() {
        let at = |ago: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - ago);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(age(at(5), now), "just now");
        assert_eq!(age(at(125), now), "2m ago");
        assert_eq!(age(at(3 * 3600 + 5), now), "3h ago");
        assert_eq!(age(at(2 * 86_400), now), "2d ago");
        // A clock that went backwards is not shown as the future.
        assert_eq!(age(now + Duration::from_secs(9), now), "just now");
    }

    #[test]
    fn keys_move_within_bounds_and_choose_or_cancel() {
        let mut picker = picker(3);
        let none = KeyModifiers::NONE;
        assert_eq!(picker.handle_key(KeyCode::Up, none), PickerOutcome::Open);
        assert_eq!(picker.selected, 0);
        for _ in 0..5 {
            picker.handle_key(KeyCode::Down, none);
        }
        assert_eq!(picker.selected, 2);
        picker.handle_key(KeyCode::Home, none);
        assert_eq!(
            picker.handle_key(KeyCode::Enter, none),
            PickerOutcome::Chosen("20261114-000000-0000".into())
        );
        assert_eq!(
            picker.handle_key(KeyCode::Esc, none),
            PickerOutcome::Cancelled
        );
        assert_eq!(
            picker.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            PickerOutcome::Cancelled
        );
    }

    #[test]
    fn rows_show_age_count_and_preview_and_scroll_to_the_selection() {
        let mut picker = picker(20);
        let lines = text(&picker.lines(60, 8, now()));
        // Four rows of chrome leave four sessions.
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[2], " ❯ just now     3 msgs  prompt 0");
        assert_eq!(lines[3], "   1m ago       3 msgs  prompt 1");
        assert!(lines[7].contains("Enter resume"));

        picker.handle_key(KeyCode::End, KeyModifiers::NONE);
        let lines = text(&picker.lines(60, 8, now()));
        assert!(lines[5].starts_with(" ❯ 19m ago"), "{lines:?}");
        assert!(lines[5].ends_with("prompt 19"));
    }

    #[test]
    fn a_long_preview_is_cut_to_the_width() {
        let picker =
            SessionPicker::new(vec![session("20261114-000000-0000", 0, &"長い".repeat(40))]);
        for line in picker.lines(40, 10, now()) {
            assert!(line.width() <= 38, "{line}");
        }
    }
}
