use crate::app::App;
use socium_core::{TranscriptEntry, TranscriptRole};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Speaker {
    User,
    Socium,
    Thinking,
    System,
}

pub(crate) struct ChatEntry {
    pub(crate) speaker: Speaker,
    pub(crate) body: String,
}

pub(crate) struct CommandOutputState {
    pub(crate) command: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) timed_out: bool,
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
        self.activity_log.push(line.into());
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

    pub(crate) fn append_streamed_thinking_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(entry) = self.messages.last_mut()
            && entry.speaker == Speaker::Thinking
        {
            // プレースホルダーが入っている場合は実際の思考内容で置き換える
            if entry.body.starts_with("Preparing response for:") {
                entry.body = text.to_string();
            } else {
                entry.body.push_str(text);
            }
        } else {
            self.messages.push(ChatEntry {
                speaker: Speaker::Thinking,
                body: text.to_string(),
            });
        }
        self.transcript_scroll = 0;
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

pub(crate) fn recent_activity_lines(entries: &[String], max_lines: usize) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }
    let start = entries.len().saturating_sub(max_lines);
    entries[start..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::{
        ChatEntry, Speaker, append_streamed_assistant_text, recent_activity_lines,
        transcript_entries_to_chat_entries, transcript_window_start,
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
    fn recent_activity_lines_keeps_latest_entries() {
        let entries = vec![
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
            "four".to_string(),
        ];

        assert_eq!(
            recent_activity_lines(&entries, 2),
            vec!["three".to_string(), "four".to_string()]
        );
        assert_eq!(recent_activity_lines(&entries, 0), Vec::<String>::new());
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
}
