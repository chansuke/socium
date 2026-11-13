//! The approval picker shown in place of the composer while a sandboxed
//! command waits for the user's decision.

use crate::app::App;
use crate::transcript::Speaker;
use ratatui::crossterm::event::KeyCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalChoice {
    AllowOnce,
    Deny,
}

impl ApprovalChoice {
    pub(crate) const ALL: [Self; 2] = [Self::AllowOnce, Self::Deny];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AllowOnce => "Allow once",
            Self::Deny => "Deny",
        }
    }
}

pub(crate) struct ApprovalPrompt {
    pub(crate) request_id: u64,
    pub(crate) tool: String,
    pub(crate) summary: String,
    pub(crate) choice: ApprovalChoice,
}

impl App {
    pub(crate) fn open_approval(&mut self, request_id: u64, tool: String, summary: String) {
        self.approval = Some(ApprovalPrompt {
            request_id,
            tool,
            summary,
            choice: ApprovalChoice::AllowOnce,
        });
        self.status = Some("Approval required: ↑↓ select · Enter confirm".into());
    }

    /// Handles a key while the picker is open. Cancelling the task is left
    /// to the caller.
    pub(crate) fn handle_approval_key(&mut self, code: KeyCode) {
        let Some(prompt) = self.approval.as_mut() else {
            return;
        };
        match code {
            KeyCode::Up | KeyCode::Char('k') => prompt.choice = ApprovalChoice::AllowOnce,
            KeyCode::Down | KeyCode::Char('j') => prompt.choice = ApprovalChoice::Deny,
            KeyCode::Enter => {
                let allow = prompt.choice == ApprovalChoice::AllowOnce;
                self.answer_approval(allow);
            }
            KeyCode::Char('1' | 'y') => self.answer_approval(true),
            KeyCode::Char('2' | 'n') => self.answer_approval(false),
            _ => {}
        }
    }

    fn answer_approval(&mut self, allow: bool) {
        let Some(prompt) = self.approval.take() else {
            return;
        };
        if let Some(task) = self.pending_task.as_ref() {
            task.control.answer_approval(prompt.request_id, allow);
        }
        let outcome = if allow {
            format!("✓ {} allowed once", prompt.tool)
        } else {
            format!("✗ {} denied", prompt.tool)
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

    fn app_with_prompt() -> App {
        let mut app = App::for_test();
        app.open_approval(7, "run_command".into(), "cargo test".into());
        app
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
    fn an_unanswered_request_closes_with_a_note() {
        let mut app = app_with_prompt();
        app.close_unanswered_approval(6);
        assert!(app.approval.is_some());

        app.close_unanswered_approval(7);
        assert!(app.approval.is_none());
        assert_eq!(last_message(&app), "✗ run_command not approved (no answer)");
    }
}
