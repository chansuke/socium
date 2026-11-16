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
use socium_core::{RewindMode, RewindPoint, Rewound};

/// Rows above and below the list inside the border: the description, a
/// blank line, a blank line, and the key help.
const CHROME_ROWS: u16 = 4;

/// What the second step offers for a prompt after which Socium changed
/// files, the default first.
const RESTORE_OPTIONS: [(RewindMode, &str); 3] = [
    (
        RewindMode::ConversationAndFiles,
        "Restore conversation and files",
    ),
    (RewindMode::Conversation, "Restore conversation only"),
    (RewindMode::Files, "Restore files only"),
];

pub(crate) struct RewindPicker {
    /// Newest first.
    pub(crate) points: Vec<RewindPoint>,
    pub(crate) selected: usize,
    step: Step,
}

enum Step {
    Prompts,
    /// Choosing what to restore for the selected prompt; the index into
    /// `RESTORE_OPTIONS`.
    Restore(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RewindOutcome {
    Open,
    /// The `turn_id` of the chosen prompt and what to restore.
    Chosen(usize, RewindMode),
    Cancelled,
}

impl RewindPicker {
    /// `points` oldest first, as the agent lists them.
    pub(crate) fn new(mut points: Vec<RewindPoint>) -> Self {
        points.reverse();
        Self {
            points,
            selected: 0,
            step: Step::Prompts,
        }
    }

    pub(crate) fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> RewindOutcome {
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            return RewindOutcome::Cancelled;
        }
        let Step::Restore(option) = &mut self.step else {
            return self.handle_prompts_key(code);
        };
        match code {
            KeyCode::Up | KeyCode::Char('k') => *option = option.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                *option = (*option + 1).min(RESTORE_OPTIONS.len() - 1);
            }
            KeyCode::Enter => {
                return RewindOutcome::Chosen(
                    self.points[self.selected].turn_id,
                    RESTORE_OPTIONS[*option].0,
                );
            }
            KeyCode::Esc | KeyCode::Char('q') => self.step = Step::Prompts,
            _ => {}
        }
        RewindOutcome::Open
    }

    fn handle_prompts_key(&mut self, code: KeyCode) -> RewindOutcome {
        let last = self.points.len().saturating_sub(1);
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::PageDown => self.selected = (self.selected + 10).min(last),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.selected = last,
            KeyCode::Enter => match self.points.get(self.selected) {
                Some(point) if point.changed_files.is_empty() => {
                    return RewindOutcome::Chosen(point.turn_id, RewindMode::Conversation);
                }
                Some(_) => self.step = Step::Restore(0),
                None => {}
            },
            KeyCode::Esc | KeyCode::Char('q') => return RewindOutcome::Cancelled,
            _ => {}
        }
        RewindOutcome::Open
    }

    /// The picker's contents inside its border, for the current step.
    pub(crate) fn lines(&self, width: u16, rows: u16) -> Vec<Line<'static>> {
        match self.step {
            Step::Prompts => self.prompt_lines(width, rows),
            Step::Restore(option) => self.restore_lines(option, width, rows),
        }
    }

    /// The prompts that fit in `rows`, one line each, scrolled to keep the
    /// selection in view.
    fn prompt_lines(&self, width: u16, rows: u16) -> Vec<Line<'static>> {
        let inner = width.saturating_sub(2) as usize;
        let visible = rows.saturating_sub(CHROME_ROWS).max(1) as usize;
        let start = self
            .selected
            .saturating_sub(visible - 1)
            .min(self.points.len().saturating_sub(visible));
        let mut lines = vec![
            Line::from(Span::styled(
                truncate_text(
                    " Go back to before a prompt and edit it. Newest first.",
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
                    let files = match point.changed_files.len() {
                        0 => String::new(),
                        1 => " · 1 file".to_string(),
                        count => format!(" · {count} files"),
                    };
                    let prompt = point
                        .prompt
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    let mut line = option_line(
                        &prompt,
                        index == self.selected,
                        inner.saturating_sub(files.chars().count()),
                    );
                    line.push_span(Span::styled(files, Style::default().fg(MUTED)));
                    line
                }),
        );
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            truncate_text(" ↑↓ select · Enter go back · Esc close", inner),
            Style::default().fg(MUTED),
        )));
        lines
    }

    /// What to restore for the selected prompt, then as many of the files
    /// that would be restored as fit in `rows`.
    fn restore_lines(&self, option: usize, width: u16, rows: u16) -> Vec<Line<'static>> {
        let inner = width.saturating_sub(2) as usize;
        let point = &self.points[self.selected];
        let muted = Style::default().fg(MUTED);
        let prompt = point
            .prompt
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut lines = vec![
            Line::from(Span::styled(
                truncate_text(&format!(" Go back to before: {prompt}"), inner),
                muted,
            )),
            Line::from(""),
        ];
        lines.extend(
            RESTORE_OPTIONS
                .iter()
                .enumerate()
                .map(|(index, (_, label))| option_line(label, index == option, inner)),
        );
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            truncate_text(" Files Socium changed from this prompt on:", inner),
            muted,
        )));
        // Everything but the file list: 5 rows above it, 2 below.
        let room = rows.saturating_sub(lines.len() as u16 + 2) as usize;
        let files = &point.changed_files;
        let shown = if files.len() > room {
            room.saturating_sub(1)
        } else {
            files.len()
        };
        lines.extend(files.iter().take(shown).map(|path| {
            Line::from(Span::styled(
                truncate_text(&format!("   {path}"), inner),
                Style::default().fg(TEXT),
            ))
        }));
        if shown < files.len() && room > 0 {
            lines.push(Line::from(Span::styled(
                format!("   … {} more", files.len() - shown),
                muted,
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            truncate_text(" ↑↓ select · Enter restore · Esc back", inner),
            muted,
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

/// A row of a list, marked and highlighted when selected, cut to `width`.
fn option_line(text: &str, selected: bool, width: usize) -> Line<'static> {
    let marker = if selected { " ❯ " } else { "   " };
    let style = if selected {
        Style::default().fg(LILAC).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT)
    };
    Line::from(vec![
        Span::styled(marker, style),
        Span::styled(
            truncate_text(text, width.saturating_sub(marker.chars().count())),
            style,
        ),
    ])
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
        self.status = Some("Rewind: ↑↓ select · Enter choose · Esc back".into());
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
            RewindOutcome::Chosen(turn_id, mode) => {
                self.rewind_picker = None;
                self.rewind(turn_id, mode);
            }
        }
    }

    fn rewind(&mut self, turn_id: usize, mode: RewindMode) {
        let Some(agent) = self.agent.as_mut() else {
            self.status = Some("Request in progress".into());
            return;
        };
        match agent.rewind(turn_id, mode) {
            Ok(rewound) => self.show_rewound(turn_id, rewound, mode),
            Err(err) => {
                self.push_message(Speaker::System, format!("Cannot rewind: {err}"));
                self.status = Some("Rewind failed".into());
            }
        }
    }

    /// Shows the conversation as the rewind left it and, when it went
    /// back, puts the prompt back in the input.
    pub(crate) fn show_rewound(&mut self, turn_id: usize, rewound: Rewound, mode: RewindMode) {
        self.show_agent_transcript();
        if mode != RewindMode::Files {
            self.activity.drop_turns_from(turn_id);
        }
        let restored = rewound.restored_files.join(", ");
        if mode == RewindMode::Files {
            self.push_message(
                Speaker::System,
                format!("Restored {restored} to before this prompt; the conversation is kept."),
            );
        } else {
            self.input = rewound.prompt;
            self.character_index = self.input.chars().count();
            self.history_index = None;
            self.history_draft.clear();
            let files = if restored.is_empty() {
                "Files were not changed; /undo reverts Socium's edits.".to_string()
            } else {
                format!("Restored {restored}.")
            };
            self.push_message(
                Speaker::System,
                format!(
                    "Went back to before this prompt; edit it and press Enter to send. {files}"
                ),
            );
        }
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
            changed_files: Vec::new(),
        }
    }

    fn with_files(turn_id: usize, prompt: &str, files: &[&str]) -> RewindPoint {
        RewindPoint {
            changed_files: files.iter().map(|file| file.to_string()).collect(),
            ..point(turn_id, prompt)
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
            RewindOutcome::Chosen(4, RewindMode::Conversation),
            "the newest prompt is listed first"
        );
        for _ in 0..5 {
            picker.handle_key(KeyCode::Down, none);
        }
        assert_eq!(
            picker.handle_key(KeyCode::Enter, none),
            RewindOutcome::Chosen(0, RewindMode::Conversation)
        );
        picker.handle_key(KeyCode::Char('k'), none);
        assert_eq!(
            picker.handle_key(KeyCode::Enter, none),
            RewindOutcome::Chosen(2, RewindMode::Conversation)
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
        app.show_rewound(
            0,
            Rewound {
                prompt: "fix the bug\nin parser.rs".into(),
                warning: None,
                restored_files: Vec::new(),
            },
            RewindMode::Conversation,
        );
        assert_eq!(app.input, "fix the bug\nin parser.rs");
        assert_eq!(app.character_index, app.input.chars().count());
        let note = &app.messages.last().unwrap().body;
        assert!(note.contains("Files were not changed"), "{note}");
        assert!(note.contains("/undo"), "{note}");
    }

    #[test]
    fn a_prompt_with_changed_files_asks_what_to_restore() {
        let none = KeyModifiers::NONE;
        let mut picker = RewindPicker::new(vec![
            point(0, "plain"),
            with_files(2, "edit things", &["a.txt", "src/b.rs"]),
        ]);
        let rows = text(&picker.lines(60, 12));
        assert_eq!(rows[2], " ❯ edit things · 2 files");
        assert_eq!(rows[3], "   plain");

        assert_eq!(picker.handle_key(KeyCode::Enter, none), RewindOutcome::Open);
        let rows = text(&picker.lines(60, 12));
        assert_eq!(
            rows[2..5],
            [
                " ❯ Restore conversation and files",
                "   Restore conversation only",
                "   Restore files only",
            ]
        );
        assert_eq!(rows[7..9], ["   a.txt", "   src/b.rs"]);

        picker.handle_key(KeyCode::Esc, none);
        assert_eq!(text(&picker.lines(60, 12))[2], " ❯ edit things · 2 files");
        picker.handle_key(KeyCode::Enter, none);
        assert_eq!(
            picker.handle_key(KeyCode::Enter, none),
            RewindOutcome::Chosen(2, RewindMode::ConversationAndFiles)
        );
        for _ in 0..3 {
            picker.handle_key(KeyCode::Char('j'), none);
        }
        assert_eq!(
            picker.handle_key(KeyCode::Enter, none),
            RewindOutcome::Chosen(2, RewindMode::Files)
        );
        picker.handle_key(KeyCode::Up, none);
        assert_eq!(
            picker.handle_key(KeyCode::Enter, none),
            RewindOutcome::Chosen(2, RewindMode::Conversation)
        );

        picker.handle_key(KeyCode::Esc, none);
        picker.handle_key(KeyCode::Down, none);
        assert_eq!(
            picker.handle_key(KeyCode::Enter, none),
            RewindOutcome::Chosen(0, RewindMode::Conversation),
            "a prompt without changed files goes back at once"
        );
    }

    #[test]
    fn a_long_file_list_is_cut_to_the_rows() {
        let files = (0..20).map(|i| format!("f{i}")).collect::<Vec<_>>();
        let mut picker = RewindPicker::new(vec![with_files(
            0,
            "p",
            &files.iter().map(String::as_str).collect::<Vec<_>>(),
        )]);
        picker.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        let rows = text(&picker.lines(40, 12));
        assert_eq!(rows.len(), 12);
        assert_eq!(rows[7..9], ["   f0", "   f1"]);
        assert_eq!(rows[9], "   … 18 more");
    }

    #[test]
    fn restoring_files_only_keeps_the_input_and_names_the_files() {
        let mut app = App::for_test();
        app.input = "draft".into();
        app.show_rewound(
            0,
            Rewound {
                prompt: "edit things".into(),
                warning: None,
                restored_files: vec!["a.txt".into(), "src/b.rs".into()],
            },
            RewindMode::Files,
        );
        assert_eq!(app.input, "draft");
        let note = &app.messages.last().unwrap().body;
        assert!(note.contains("Restored a.txt, src/b.rs"), "{note}");

        app.show_rewound(
            0,
            Rewound {
                prompt: "edit things".into(),
                warning: None,
                restored_files: vec!["a.txt".into()],
            },
            RewindMode::ConversationAndFiles,
        );
        assert_eq!(app.input, "edit things");
        let note = &app.messages.last().unwrap().body;
        assert!(note.contains("Restored a.txt."), "{note}");
    }
}
