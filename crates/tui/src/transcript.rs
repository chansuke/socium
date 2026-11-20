use crate::app::App;
use socium_core::{TranscriptEntry, TranscriptRole};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Speaker {
    User,
    Socium,
    Thinking,
    System,
    /// A tool call: name and argument, then its result on the next line.
    Tool {
        failed: bool,
    },
}

pub(crate) struct ChatEntry {
    pub(crate) speaker: Speaker,
    pub(crate) body: String,
}

pub(crate) struct CommandOutputState {
    pub(crate) command: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) timed_out: bool,
    /// Either stream was cut in what the model got; the saved files hold
    /// all of it.
    pub(crate) truncated: bool,
    /// The saved streams' path without `.stdout` or `.stderr`.
    pub(crate) saved_output: Option<String>,
    pub(crate) stdout_preview: String,
    pub(crate) stderr_preview: String,
}

impl App {
    pub(crate) fn push_message(&mut self, speaker: Speaker, body: impl Into<String>) {
        self.messages.push(ChatEntry {
            speaker,
            body: body.into(),
        });
        self.transcript_scroll = 0;
    }

    pub(crate) fn push_activity(&mut self, line: impl Into<String>) {
        self.activity.push_note(line);
    }

    pub(crate) fn append_streamed_assistant_text(&mut self, text: &str) {
        append_streamed_assistant_text(&mut self.messages, text);
        self.transcript_scroll = 0;
    }

    pub(crate) fn pop_thinking_message(&mut self) {
        if self
            .messages
            .last()
            .is_some_and(|entry| entry.speaker == Speaker::Thinking)
        {
            self.messages.pop();
        }
    }

    /// Shows the one-line thinking indicator; the thoughts themselves are
    /// in the Activity pane.
    pub(crate) fn show_thinking_indicator(&mut self) {
        if self
            .messages
            .last()
            .is_none_or(|entry| entry.speaker != Speaker::Thinking)
        {
            self.push_message(Speaker::Thinking, String::new());
        }
    }
}

fn append_streamed_assistant_text(messages: &mut Vec<ChatEntry>, text: &str) {
    if text.is_empty() {
        return;
    }

    if let Some(entry) = messages.last_mut()
        && entry.speaker == Speaker::Socium
    {
        entry.body.push_str(text);
        return;
    }

    messages.push(ChatEntry {
        speaker: Speaker::Socium,
        body: text.to_string(),
    });
}

pub(crate) fn transcript_entries_to_chat_entries(entries: Vec<TranscriptEntry>) -> Vec<ChatEntry> {
    entries
        .into_iter()
        .map(|entry| ChatEntry {
            speaker: match entry.role {
                TranscriptRole::User => Speaker::User,
                TranscriptRole::Assistant => Speaker::Socium,
                TranscriptRole::System => Speaker::System,
                TranscriptRole::Tool { failed } => Speaker::Tool { failed },
            },
            body: entry.text,
        })
        .collect()
}

pub(crate) fn transcript_window_start(total_lines: usize, height: usize, scroll: usize) -> usize {
    let visible_height = height.max(1);
    let max_start = total_lines.saturating_sub(visible_height);
    max_start.saturating_sub(scroll.min(max_start))
}

#[cfg(test)]
mod tests {
    use super::{
        ChatEntry, Speaker, append_streamed_assistant_text, transcript_entries_to_chat_entries,
        transcript_window_start,
    };
    use socium_core::{TranscriptEntry, TranscriptRole};

    #[test]
    fn transcript_entries_map_to_chat_speakers() {
        let entries = vec![
            TranscriptEntry {
                role: TranscriptRole::User,
                text: "u".to_string(),
            },
            TranscriptEntry {
                role: TranscriptRole::Assistant,
                text: "a".to_string(),
            },
            TranscriptEntry {
                role: TranscriptRole::System,
                text: "s".to_string(),
            },
        ];

        let chat = transcript_entries_to_chat_entries(entries);

        assert_eq!(chat.len(), 3);
        assert!(matches!(chat[0].speaker, Speaker::User));
        assert!(matches!(chat[1].speaker, Speaker::Socium));
        assert!(matches!(chat[2].speaker, Speaker::System));
    }

    #[test]
    fn transcript_window_start_respects_scroll_offset() {
        assert_eq!(transcript_window_start(20, 5, 0), 15);
        assert_eq!(transcript_window_start(20, 5, 3), 12);
        assert_eq!(transcript_window_start(20, 5, 99), 0);
        assert_eq!(transcript_window_start(3, 10, 5), 0);
    }

    #[test]
    fn append_streamed_assistant_text_appends_to_latest_assistant_entry() {
        let mut messages = vec![
            ChatEntry {
                speaker: Speaker::User,
                body: "user".to_string(),
            },
            ChatEntry {
                speaker: Speaker::Socium,
                body: "hel".to_string(),
            },
        ];

        append_streamed_assistant_text(&mut messages, "lo");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].body, "hello");
    }

    #[test]
    fn append_streamed_assistant_text_pushes_new_entry_after_system_message() {
        let mut messages = vec![ChatEntry {
            speaker: Speaker::System,
            body: "tool finished".to_string(),
        }];

        append_streamed_assistant_text(&mut messages, "answer");

        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[1].speaker, Speaker::Socium));
        assert_eq!(messages[1].body, "answer");
    }

    #[test]
    fn a_recorded_tool_call_replaces_the_thinking_indicator() {
        let mut app = crate::app::App::for_test();
        app.show_thinking_indicator();
        app.apply_progress(socium_core::AgentProgress::ToolCallRecorded(
            TranscriptEntry {
                role: TranscriptRole::Tool { failed: true },
                text: "run_command cargo test\nexit code 101".into(),
            },
        ));
        assert_eq!(app.messages.len(), 1);
        assert!(app.messages[0].speaker == Speaker::Tool { failed: true });
    }
}
