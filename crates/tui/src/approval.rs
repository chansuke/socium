//! The approval picker shown in place of the composer while a tool call
//! waits for the user's decision, and the request details it shows in
//! place of the transcript. Reading keys only scroll the details; only
//! Enter, the number keys, and y/a/n answer.

use crate::app::App;
use crate::transcript::Speaker;
use ratatui::crossterm::event::KeyCode;
use socium_core::ApprovalAnswer;
use std::cell::Cell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalChoice {
    AllowOnce,
    /// Allow, and stop asking about what the request offered.
    AllowForSession,
    Deny,
}

impl ApprovalChoice {
    fn answer(self) -> ApprovalAnswer {
        match self {
            Self::AllowOnce => ApprovalAnswer::AllowOnce,
            Self::AllowForSession => ApprovalAnswer::AllowForSession,
            Self::Deny => ApprovalAnswer::Deny,
        }
    }
}

pub(crate) struct ApprovalPrompt {
    pub(crate) request_id: u64,
    pub(crate) tool: String,
    pub(crate) summary: String,
    /// What "allow for this session" would cover, when the request offers
    /// it; see `socium_core::Grant::scope`.
    pub(crate) session_option: Option<String>,
    pub(crate) choice: ApprovalChoice,
    /// The whole request, Git diff included, as the user reviews it.
    pub(crate) details: String,
    /// First details row shown.
    pub(crate) scroll: usize,
    /// Rows the details pane showed when last drawn and the largest useful
    /// scroll offset, so paging matches what is on screen.
    pub(crate) viewport: Cell<DetailsViewport>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DetailsViewport {
    pub(crate) rows: usize,
    pub(crate) max_scroll: usize,
}

impl ApprovalPrompt {
    /// The choices in the order shown; their positions are the number keys.
    pub(crate) fn choices(&self) -> Vec<ApprovalChoice> {
        let mut choices = vec![ApprovalChoice::AllowOnce];
        if self.session_option.is_some() {
            choices.push(ApprovalChoice::AllowForSession);
        }
        choices.push(ApprovalChoice::Deny);
        choices
    }

    pub(crate) fn label(&self, choice: ApprovalChoice) -> String {
        match choice {
            ApprovalChoice::AllowOnce => "Allow once".into(),
            ApprovalChoice::AllowForSession => format!(
                "Allow {} until Socium exits",
                self.session_option.as_deref().unwrap_or("this")
            ),
            ApprovalChoice::Deny => "Deny (tell Socium what to do instead)".into(),
        }
    }

    fn move_choice(&mut self, step: isize) {
        let choices = self.choices();
        let at = choices
            .iter()
            .position(|choice| *choice == self.choice)
            .unwrap_or(0);
        let next = at.saturating_add_signed(step).min(choices.len() - 1);
        self.choice = choices[next];
    }

    fn scroll_by(&mut self, rows: isize) {
        let max = self.viewport.get().max_scroll;
        self.scroll = self.scroll.saturating_add_signed(rows).min(max);
    }

    fn page(&self) -> isize {
        self.viewport.get().rows.saturating_sub(1).max(1) as isize
    }
}

impl App {
    pub(crate) fn open_approval(
        &mut self,
        request_id: u64,
        tool: String,
        summary: String,
        details: String,
        session_option: Option<String>,
    ) {
        self.approval = Some(ApprovalPrompt {
            request_id,
            tool,
            summary,
            session_option,
            choice: ApprovalChoice::AllowOnce,
            details,
            scroll: 0,
            viewport: Cell::default(),
        });
        self.status =
            Some("Approval required: ↑↓ select · Enter confirm · PgUp/PgDn/Home/End review".into());
    }

    /// Handles a key while the picker is open. Cancelling the task is left
    /// to the caller.
    pub(crate) fn handle_approval_key(&mut self, code: KeyCode) {
        let Some(prompt) = self.approval.as_mut() else {
            return;
        };
        match code {
            KeyCode::PageDown | KeyCode::Char(' ') => prompt.scroll_by(prompt.page()),
            KeyCode::PageUp | KeyCode::Char('b') => prompt.scroll_by(-prompt.page()),
            KeyCode::Home | KeyCode::Char('g') => prompt.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => prompt.scroll = prompt.viewport.get().max_scroll,
            KeyCode::Up | KeyCode::Char('k') => prompt.move_choice(-1),
            KeyCode::Down | KeyCode::Char('j') => prompt.move_choice(1),
            KeyCode::Enter => {
                let choice = prompt.choice;
                self.answer_approval(choice);
            }
            KeyCode::Char('y') => self.answer_approval(ApprovalChoice::AllowOnce),
            KeyCode::Char('a') if prompt.session_option.is_some() => {
                self.answer_approval(ApprovalChoice::AllowForSession)
            }
            KeyCode::Char('n') => self.answer_approval(ApprovalChoice::Deny),
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if let Some(choice) = prompt.choices().get(index).copied() {
                    self.answer_approval(choice);
                }
            }
            _ => {}
        }
    }

    fn answer_approval(&mut self, choice: ApprovalChoice) {
        let Some(prompt) = self.approval.take() else {
            return;
        };
        if let Some(task) = self.pending_task.as_ref() {
            task.control.answer(prompt.request_id, choice.answer());
        }
        let outcome = match choice {
            ApprovalChoice::AllowOnce => format!("✓ {} allowed once", prompt.tool),
            ApprovalChoice::AllowForSession => format!(
                "✓ {} allowed; {} allowed until Socium exits",
                prompt.tool,
                prompt.session_option.as_deref().unwrap_or("this")
            ),
            ApprovalChoice::Deny => format!("✗ {} denied", prompt.tool),
        };
        self.push_message(Speaker::System, outcome);
        self.status = Some("Approval answered".into());
    }

    /// Closes the picker when the request ended without an answer, such as
    /// after a timeout.
    pub(crate) fn close_unanswered_approval(&mut self, request_id: u64) {
        if self
            .approval
            .as_ref()
            .is_some_and(|prompt| prompt.request_id == request_id)
            && let Some(prompt) = self.approval.take()
        {
            self.push_message(
                Speaker::System,
                format!("✗ {} not approved (no answer)", prompt.tool),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use socium_core::Grant;

    fn app_with_prompt() -> App {
        let mut app = App::for_test();
        app.open_approval(
            7,
            "run_command".into(),
            "cargo test".into(),
            "Tool call: c1".into(),
            None,
        );
        app
    }

    fn app_with_session_option() -> App {
        let mut app = App::for_test();
        app.open_approval(
            8,
            "run_command".into(),
            "cargo test".into(),
            "Tool call: c2".into(),
            Some(Grant::Command(vec!["cargo".into(), "test".into()]).scope()),
        );
        app
    }

    #[test]
    fn a_request_that_offers_it_can_be_allowed_for_the_session() {
        let mut app = app_with_session_option();
        let prompt = app.approval.as_ref().unwrap();
        assert_eq!(
            prompt.label(ApprovalChoice::AllowForSession),
            "Allow `cargo test` commands until Socium exits"
        );
        assert_eq!(
            prompt.choices(),
            [
                ApprovalChoice::AllowOnce,
                ApprovalChoice::AllowForSession,
                ApprovalChoice::Deny
            ]
        );
        app.handle_approval_key(KeyCode::Down);
        assert_eq!(
            app.approval.as_ref().unwrap().choice,
            ApprovalChoice::AllowForSession
        );
        app.handle_approval_key(KeyCode::Enter);
        assert_eq!(
            last_message(&app),
            "✓ run_command allowed; `cargo test` commands allowed until Socium exits"
        );

        for (key, expected) in [
            (KeyCode::Char('2'), "allowed until Socium exits"),
            (KeyCode::Char('a'), "allowed until Socium exits"),
            (KeyCode::Char('3'), "denied"),
        ] {
            let mut app = app_with_session_option();
            app.handle_approval_key(key);
            assert!(last_message(&app).contains(expected), "{key:?}");
        }
        // Without the offer, 'a' does nothing and 2 denies.
        let mut app = app_with_prompt();
        app.handle_approval_key(KeyCode::Char('a'));
        assert!(app.approval.is_some());
    }

    #[test]
    fn an_mcp_tool_can_be_allowed_for_the_session() {
        let mut app = App::for_test();
        app.open_approval(
            9,
            "mcp__github__get_issue".into(),
            r#"{"issue":7}"#.into(),
            "Tool call: c3".into(),
            Some(Grant::McpTool("mcp__github__get_issue".into()).scope()),
        );
        let prompt = app.approval.as_ref().unwrap();
        assert_eq!(
            prompt.label(ApprovalChoice::AllowForSession),
            "Allow the MCP tool `mcp__github__get_issue` until Socium exits"
        );
        app.handle_approval_key(KeyCode::Char('a'));
        assert_eq!(
            last_message(&app),
            "✓ mcp__github__get_issue allowed; the MCP tool `mcp__github__get_issue` allowed until Socium exits"
        );
    }

    fn last_message(app: &App) -> &str {
        &app.messages.last().unwrap().body
    }

    #[test]
    fn arrows_move_the_choice_and_enter_confirms_it() {
        let mut app = app_with_prompt();
        app.handle_approval_key(KeyCode::Down);
        assert_eq!(app.approval.as_ref().unwrap().choice, ApprovalChoice::Deny);
        app.handle_approval_key(KeyCode::Down);
        assert_eq!(app.approval.as_ref().unwrap().choice, ApprovalChoice::Deny);

        app.handle_approval_key(KeyCode::Enter);
        assert!(app.approval.is_none());
        assert_eq!(last_message(&app), "✗ run_command denied");
    }

    #[test]
    fn number_and_letter_shortcuts_answer_at_once() {
        for (key, expected) in [
            (KeyCode::Char('1'), "✓ run_command allowed once"),
            (KeyCode::Char('y'), "✓ run_command allowed once"),
            (KeyCode::Char('2'), "✗ run_command denied"),
            (KeyCode::Char('n'), "✗ run_command denied"),
        ] {
            let mut app = app_with_prompt();
            app.handle_approval_key(key);
            assert!(app.approval.is_none());
            assert_eq!(last_message(&app), expected);
        }
    }

    #[test]
    fn other_keys_leave_the_prompt_open() {
        let mut app = app_with_prompt();
        app.handle_approval_key(KeyCode::Char('x'));
        assert!(app.approval.is_some());
        assert!(app.messages.is_empty());
    }

    #[test]
    fn reading_keys_scroll_the_details_without_answering() {
        let mut app = app_with_prompt();
        let prompt = app.approval.as_mut().unwrap();
        prompt.viewport.set(DetailsViewport {
            rows: 10,
            max_scroll: 25,
        });
        prompt.choice = ApprovalChoice::Deny;

        for (key, scroll) in [
            (KeyCode::PageDown, 9),
            (KeyCode::Char(' '), 18),
            (KeyCode::PageDown, 25),
            (KeyCode::PageUp, 16),
            (KeyCode::Home, 0),
            (KeyCode::End, 25),
            (KeyCode::Char('b'), 16),
            (KeyCode::Char('g'), 0),
        ] {
            app.handle_approval_key(key);
            let prompt = app.approval.as_ref().expect("reading never answers");
            assert_eq!(prompt.scroll, scroll, "{key:?}");
            assert_eq!(prompt.choice, ApprovalChoice::Deny, "{key:?}");
        }
        assert!(app.messages.is_empty());

        // The answer still applies to the same one-time request.
        app.handle_approval_key(KeyCode::Enter);
        assert!(app.approval.is_none());
        assert_eq!(last_message(&app), "✗ run_command denied");
    }

    #[test]
    fn an_unanswered_request_closes_with_a_note() {
        let mut app = app_with_prompt();
        app.close_unanswered_approval(6);
        assert!(app.approval.is_some());

        app.close_unanswered_approval(7);
        assert!(app.approval.is_none());
        assert_eq!(last_message(&app), "✗ run_command not approved (no answer)");
    }
}
