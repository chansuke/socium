//! The session picker shown by `-r` and `/resume`, in place of the
//! conversation, and resuming the chosen session.

use crate::app::App;
use crate::model_picker::model_label;
use crate::transcript::Speaker;
use crate::view::text_layout::truncate_text;
use crate::view::theme::*;
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyModifiers},
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
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
    /// `d` was pressed on the selection; `y` confirms the deletion.
    pub(crate) confirming_delete: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PickerOutcome {
    Open,
    Chosen(String),
    Delete(String),
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
            confirming_delete: false,
        }
    }

    /// Drops a deleted session from the list.
    pub(crate) fn remove(&mut self, id: &str) {
        self.sessions.retain(|session| session.id != id);
        self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
    }

    pub(crate) fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> PickerOutcome {
        if std::mem::take(&mut self.confirming_delete) {
            return match (code, self.sessions.get(self.selected)) {
                (KeyCode::Char('y'), Some(session)) => PickerOutcome::Delete(session.id.clone()),
                _ => PickerOutcome::Open,
            };
        }
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
            KeyCode::Char('d') | KeyCode::Delete => {
                self.confirming_delete = !self.sessions.is_empty();
            }
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
                    let name = session
                        .name
                        .as_ref()
                        .map(|name| format!("{name} · "))
                        .unwrap_or_default();
                    let name = truncate_text(&name, inner.saturating_sub(head.width()));
                    let preview = if session.preview.is_empty() {
                        format!("({})", session.id)
                    } else {
                        session.preview.clone()
                    };
                    let preview =
                        truncate_text(&preview, inner.saturating_sub(head.width() + name.width()));
                    let (head_style, preview_style) = if !session.readable {
                        (Style::default().fg(MUTED), Style::default().fg(MUTED))
                    } else if selected {
                        (
                            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                            Style::default().fg(TEXT),
                        )
                    } else {
                        (Style::default().fg(MUTED), Style::default().fg(TEXT))
                    };
                    Line::from(vec![
                        Span::styled(head, head_style),
                        Span::styled(name, preview_style.add_modifier(Modifier::BOLD)),
                        Span::styled(preview, preview_style),
                    ])
                }),
        );
        lines.push(Line::from(""));
        lines.push(match self.sessions.get(self.selected) {
            Some(session) if self.confirming_delete => Line::from(Span::styled(
                truncate_text(
                    &format!(
                        " Delete {} permanently? y to delete · any other key to keep",
                        session.id
                    ),
                    inner,
                ),
                Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
            )),
            _ => Line::from(Span::styled(
                truncate_text(" ↑↓ select · Enter resume · d delete · Esc close", inner),
                Style::default().fg(MUTED),
            )),
        });
        lines
    }

    /// Draws the picker over `area`, in place of the conversation.
    pub(crate) fn draw(&self, frame: &mut Frame, area: Rect, now: SystemTime) {
        let title = self
            .sessions
            .get(self.selected)
            .map(|session| format!(" Resume · {} ", session.id))
            .unwrap_or_else(|| " Resume ".to_string());
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(title)
            .border_style(Style::default().fg(FOCUS_BORDER))
            .style(Style::default().bg(BG));
        let rows = area.height.saturating_sub(2);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(self.lines(area.width, rows, now)).block(block),
            area,
        );
    }
}

impl App {
    /// Lists the other saved sessions to choose one to resume.
    pub(crate) fn open_session_picker(&mut self) {
        let Some(agent) = self.agent.as_mut() else {
            self.status = Some("Request in progress".into());
            return;
        };
        match agent.list_sessions() {
            Ok(sessions) if sessions.is_empty() => {
                self.push_message(
                    Speaker::System,
                    "No other saved conversation in this workspace",
                );
            }
            Ok(sessions) => {
                self.session_picker = Some(SessionPicker::new(sessions));
                self.status = Some("Resume: ↑↓ select · Enter resume · Esc close".into());
            }
            Err(err) => {
                self.push_message(Speaker::System, format!("Cannot list sessions: {err}"));
            }
        }
    }

    /// Returns `true` when the resumed history should be compacted before
    /// the next turn.
    pub(crate) fn handle_session_picker_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        let Some(picker) = self.session_picker.as_mut() else {
            return false;
        };
        match picker.handle_key(code, modifiers) {
            PickerOutcome::Open => false,
            PickerOutcome::Cancelled => {
                self.session_picker = None;
                self.status = Some("Session unchanged".into());
                false
            }
            PickerOutcome::Chosen(id) => {
                self.session_picker = None;
                self.resume_session(&id)
            }
            PickerOutcome::Delete(id) => {
                self.delete_session(&id);
                false
            }
        }
    }

    fn delete_session(&mut self, id: &str) {
        let Some(agent) = self.agent.as_mut() else {
            self.status = Some("Request in progress".into());
            return;
        };
        match agent.delete_session(id) {
            Ok(()) => {
                self.status = Some(format!("Deleted session {id}"));
                if let Some(picker) = self.session_picker.as_mut() {
                    picker.remove(id);
                    if picker.sessions.is_empty() {
                        self.session_picker = None;
                        self.push_message(
                            Speaker::System,
                            "No other saved conversation in this workspace",
                        );
                    }
                }
            }
            Err(err) => self.status = Some(format!("Cannot delete {id}: {err}")),
        }
    }

    /// `/rename <name>` names the open session; without a name it shows
    /// the current one.
    pub(crate) fn rename_session(&mut self, name: Option<&str>) {
        let Some(agent) = self.agent.as_mut() else {
            self.status = Some("Request in progress".into());
            return;
        };
        let message = match name {
            None => match agent.session_name() {
                Some(name) => format!(
                    "Session {} is named '{name}'; /rename <name> changes it",
                    agent.session_id()
                ),
                None => format!(
                    "Session {} has no name; /rename <name> names it",
                    agent.session_id()
                ),
            },
            Some(name) => match agent.rename_session(name) {
                Ok(()) => {
                    let name = agent.session_name().unwrap_or_default();
                    format!("Named this session '{name}'; resume it with -r '{name}'")
                }
                Err(err) => format!("Cannot rename: {err}"),
            },
        };
        self.push_message(Speaker::System, message);
    }

    /// Returns `true` when the resumed history should be compacted before
    /// the next turn.
    pub(crate) fn resume_session(&mut self, query: &str) -> bool {
        let Some(agent) = self.agent.as_mut() else {
            self.status = Some("Request in progress".into());
            return false;
        };
        match agent.resume_session(query) {
            Ok(switch) => {
                self.load_session_view();
                self.model_label = model_label(&switch.model);
                for warning in switch.warnings {
                    self.push_message(Speaker::System, warning);
                }
                self.status = Some("Resumed".into());
                if switch.needs_compaction {
                    self.push_message(Speaker::System, "Compacting conversation context…");
                    self.status = Some("Compacting".into());
                }
                switch.needs_compaction
            }
            Err(err) => {
                self.push_message(Speaker::System, format!("Cannot resume {query}: {err}"));
                self.status = Some("Session unchanged".into());
                false
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
            name: None,
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

    #[test]
    fn resuming_while_a_request_runs_is_refused() {
        let mut app = App::for_test();
        app.open_session_picker();
        assert!(app.session_picker.is_none());
        assert!(!app.resume_session("20261114"));
        assert_eq!(app.status.as_deref(), Some("Request in progress"));
    }

    #[test]
    fn closing_the_picker_keeps_the_session() {
        let mut app = App::for_test();
        app.session_picker = Some(picker(2));
        assert!(!app.handle_session_picker_key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.session_picker.is_none());
        assert_eq!(app.status.as_deref(), Some("Session unchanged"));
    }

    #[test]
    fn d_then_y_deletes_the_selection_and_any_other_key_keeps_it() {
        let mut picker = picker(3);
        let none = KeyModifiers::NONE;
        picker.handle_key(KeyCode::Down, none);
        assert_eq!(
            picker.handle_key(KeyCode::Char('d'), none),
            PickerOutcome::Open
        );
        let lines = text(&picker.lines(80, 10, now()));
        assert!(
            lines
                .last()
                .unwrap()
                .contains("Delete 20261114-000001-0000 permanently?"),
            "{lines:?}"
        );
        assert_eq!(
            picker.handle_key(KeyCode::Char('n'), none),
            PickerOutcome::Open
        );
        assert!(!picker.confirming_delete);
        // Esc while confirming keeps the picker open.
        picker.handle_key(KeyCode::Char('d'), none);
        assert_eq!(picker.handle_key(KeyCode::Esc, none), PickerOutcome::Open);

        picker.handle_key(KeyCode::Char('d'), none);
        assert_eq!(
            picker.handle_key(KeyCode::Char('y'), none),
            PickerOutcome::Delete("20261114-000001-0000".into())
        );
        picker.remove("20261114-000001-0000");
        assert_eq!(picker.sessions.len(), 2);
        assert_eq!(picker.selected, 1);
        picker.remove("20261114-000002-0000");
        assert_eq!(picker.selected, 0);
    }

    #[test]
    fn a_named_session_shows_its_name_before_the_preview() {
        let mut named = session("20261114-000000-0000", 0, "first prompt");
        named.name = Some("readme fix".into());
        let picker = SessionPicker::new(vec![named]);
        let lines = text(&picker.lines(80, 10, now()));
        assert!(
            lines[2].ends_with("3 msgs  readme fix · first prompt"),
            "{lines:?}"
        );
    }
}
