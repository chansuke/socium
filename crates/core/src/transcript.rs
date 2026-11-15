//! The conversation as the transcript shows it, rebuilt from saved
//! messages and turn events: prompts, answers, one entry per tool call
//! with its result, and failed turns.

use crate::activity::{summarize_tool_input, summarize_tool_result};
use crate::agent::types::{ContentBlock, Message, MessageContent, Role};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptRole {
    User,
    Assistant,
    System,
    /// A tool call: its name and main argument on the first line, then a
    /// one-line result.
    Tool {
        failed: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    pub role: TranscriptRole,
    pub text: String,
}

/// The tool calls that ran after a human approved them, as the turn
/// records them at the execution boundary.
const HUMAN_APPROVED: &[&str] = &[crate::tools::HUMAN_APPROVED, "human-approved-once"];

/// One tool call and its result. `approved` marks a call that ran after
/// the human allowed it.
pub(crate) fn tool_entry(
    name: &str,
    input: &Value,
    result: Option<(&str, bool)>,
    approved: bool,
) -> TranscriptEntry {
    let summary = summarize_tool_input(name, input);
    let mut text = if summary.is_empty() {
        name.to_string()
    } else {
        format!("{name} {summary}")
    };
    if approved {
        text.push_str(" (approved)");
    }
    let failed = match result {
        Some((content, failed)) => {
            let line = summarize_tool_result(content);
            if !line.is_empty() {
                text.push('\n');
                text.push_str(&line);
            }
            if !failed {
                text.push_str(&diff_preview(content));
            }
            failed
        }
        None => {
            text.push_str("\nNo result was saved");
            true
        }
    };
    TranscriptEntry {
        role: TranscriptRole::Tool { failed },
        text,
    }
}

/// Diff lines an edit shows in the conversation; the tool result has more.
const PREVIEW_DIFF_LINES: usize = 12;

/// What an edit changed, from the "Changed:" summary and the unified diff
/// that edit results end with: the counts, then the first changed lines,
/// each on its own line after a newline. Empty for other results.
fn diff_preview(content: &str) -> String {
    let Some(start) = content.find("\nChanged: ") else {
        return String::new();
    };
    let mut lines = content[start + 1..].lines();
    let mut preview = format!("\n{}", lines.next().unwrap_or_default());
    let body = lines
        .filter(|line| !line.starts_with("--- ") && !line.starts_with("+++ "))
        .take_while(|line| line.starts_with(['@', '+', '-', ' ', '\\']))
        .collect::<Vec<_>>();
    for line in body.iter().take(PREVIEW_DIFF_LINES) {
        preview.push('\n');
        preview.push_str(line);
    }
    if body.len() > PREVIEW_DIFF_LINES {
        preview.push_str(&format!(
            "\n… {} more diff lines",
            body.len() - PREVIEW_DIFF_LINES
        ));
    }
    preview
}

/// The event's own record: checkpoints nest it under `event`.
fn event_body(event: &Value) -> &Value {
    event.get("event").unwrap_or(event)
}

pub(crate) fn transcript_entries(messages: &[Message], events: &[Value]) -> Vec<TranscriptEntry> {
    let mut results = HashMap::new();
    for message in messages {
        if let MessageContent::Blocks(blocks) = &message.content {
            for block in blocks {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = block
                {
                    results.insert(
                        tool_use_id.as_str(),
                        (content.as_str(), *is_error == Some(true)),
                    );
                }
            }
        }
    }
    let mut approved = HashSet::new();
    let mut turn_starts = HashSet::new();
    let mut failures = BTreeMap::new();
    for event in events {
        let body = event_body(event);
        match body["type"].as_str() {
            Some("tool_started")
                if body["authorization"]
                    .as_str()
                    .is_some_and(|value| HUMAN_APPROVED.contains(&value)) =>
            {
                if let Some(id) = body["tool_call_id"].as_str() {
                    approved.insert(id);
                }
            }
            Some("turn_started") => {
                if let Some(turn) = event["turn_id"].as_u64() {
                    turn_starts.insert(turn as usize);
                }
            }
            Some("turn_failed") => {
                if let Some(turn) = event["turn_id"].as_u64() {
                    let error = event["error"].as_str().unwrap_or("unknown error");
                    failures.insert(turn as usize, format!("Request failed: {error}"));
                }
            }
            _ => {}
        }
    }

    let mut entries = Vec::new();
    let flush_failures = |entries: &mut Vec<TranscriptEntry>,
                          failures: &mut BTreeMap<usize, String>,
                          before: usize| {
        let later = failures.split_off(&before);
        for (_, text) in std::mem::replace(failures, later) {
            entries.push(TranscriptEntry {
                role: TranscriptRole::System,
                text,
            });
        }
    };
    for (index, message) in messages.iter().enumerate() {
        // A turn's failure shows after everything the turn produced.
        if turn_starts.contains(&index) {
            flush_failures(&mut entries, &mut failures, index);
        }
        let role = match message.role {
            Role::User => TranscriptRole::User,
            Role::Assistant => TranscriptRole::Assistant,
            Role::System => TranscriptRole::System,
        };
        let mut text = String::new();
        let push_text = |entries: &mut Vec<TranscriptEntry>, text: &mut String| {
            if !text.trim().is_empty() {
                entries.push(TranscriptEntry {
                    role: role.clone(),
                    text: std::mem::take(text),
                });
            }
            text.clear();
        };
        match &message.content {
            MessageContent::Text(body) => text.push_str(body),
            MessageContent::Blocks(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::Text { text: body } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(body);
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            push_text(&mut entries, &mut text);
                            entries.push(tool_entry(
                                name,
                                input,
                                results.get(id.as_str()).copied(),
                                approved.contains(id.as_str()),
                            ));
                        }
                        // Results are shown with their calls; thinking is
                        // in the Activity pane.
                        _ => {}
                    }
                }
            }
        }
        push_text(&mut entries, &mut text);
    }
    flush_failures(&mut entries, &mut failures, usize::MAX);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_use(id: &str, name: &str, input: Value) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }
    }

    fn tool_result(id: &str, content: &str, failed: bool) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: content.into(),
            is_error: failed.then_some(true),
        }
    }

    fn entry(role: TranscriptRole, text: &str) -> TranscriptEntry {
        TranscriptEntry {
            role,
            text: text.into(),
        }
    }

    #[test]
    fn an_edit_shows_its_counts_and_first_diff_lines() {
        // Review N13: an edit appeared as one "Path: …" line.
        let mut diff = String::from(
            "Updated file: a.rs\nChanged: a.rs (+20 -1)\n--- a.rs\n+++ a.rs\n@@ -1 +1,20 @@\n-old\n",
        );
        for n in 0..20 {
            diff.push_str(&format!("+new {n}\n"));
        }
        let entry = tool_entry(
            "apply_patch",
            &json!({"patch": "*** Begin Patch"}),
            Some((&diff, false)),
            false,
        );
        let lines = entry.text.lines().collect::<Vec<_>>();
        assert_eq!(lines[1], "Updated file: a.rs");
        assert_eq!(lines[2], "Changed: a.rs (+20 -1)");
        assert_eq!(lines[3], "@@ -1 +1,20 @@");
        assert_eq!(lines[4], "-old");
        assert_eq!(lines.len(), 3 + 12 + 1);
        assert_eq!(lines.last().unwrap(), &"… 10 more diff lines");

        let plain = tool_entry(
            "read_file",
            &json!({"path": "a.rs"}),
            Some(("Path: a.rs", false)),
            false,
        );
        assert_eq!(plain.text.lines().count(), 2);
    }

    #[test]
    fn tool_calls_show_with_their_results_approval_and_failures() {
        let messages = vec![
            Message::user_text("fix the test".into()),
            Message::assistant_blocks(vec![
                ContentBlock::Thinking {
                    thinking: "plan".into(),
                    signature: "sig".into(),
                },
                ContentBlock::Text {
                    text: "Looking.".into(),
                },
                tool_use("a", "read_file", json!({"path": "src/lib.rs"})),
                tool_use("b", "run_command", json!({"command": ["cargo", "test"]})),
                tool_use("c", "run_command", json!({"command": ["rm", "-rf", "x"]})),
            ]),
            Message::user_blocks(vec![
                tool_result("a", "Path: src/lib.rs\nfn main() {}", false),
                tool_result("b", "exit code 101\ntest failed", true),
                tool_result(
                    "c",
                    "Sandbox command denied: explicit human approval required",
                    true,
                ),
            ]),
            Message::assistant_blocks(vec![ContentBlock::Text {
                text: "One test fails.".into(),
            }]),
        ];
        let events = vec![
            json!({"type": "turn_started", "turn_id": 0, "sequence": 0}),
            json!({"sequence": 1, "turn_id": 0, "event": {"type": "tool_started", "tool_call_id": "a", "authorization": "workspace-file-policy"}}),
            json!({"sequence": 2, "turn_id": 0, "event": {"type": "tool_started", "tool_call_id": "b", "authorization": "human-approved-once"}}),
        ];

        assert_eq!(
            transcript_entries(&messages, &events),
            [
                entry(TranscriptRole::User, "fix the test"),
                entry(TranscriptRole::Assistant, "Looking."),
                entry(
                    TranscriptRole::Tool { failed: false },
                    "read_file src/lib.rs\nPath: src/lib.rs"
                ),
                entry(
                    TranscriptRole::Tool { failed: true },
                    "run_command cargo test (approved)\nexit code 101"
                ),
                entry(
                    TranscriptRole::Tool { failed: true },
                    "run_command rm -rf x\nSandbox command denied: explicit human approval required"
                ),
                entry(TranscriptRole::Assistant, "One test fails."),
            ]
        );
    }

    #[test]
    fn a_failed_turn_shows_after_what_it_produced_and_before_the_next_prompt() {
        let messages = vec![
            Message::user_text("first".into()),
            Message::assistant_blocks(vec![tool_use("a", "list_files", json!({"path": "."}))]),
            Message::user_blocks(vec![tool_result(
                "a",
                "Interrupted before result was saved. Outcome unknown; inspect workspace before retrying.",
                true,
            )]),
            Message::user_text("second".into()),
            Message::user_text("third".into()),
        ];
        let events = vec![
            json!({"type": "turn_started", "turn_id": 0, "sequence": 0}),
            json!({"type": "turn_failed", "turn_id": 0, "sequence": 1, "error": "Cancelled"}),
            json!({"type": "turn_started", "turn_id": 3, "sequence": 2}),
            json!({"type": "turn_failed", "turn_id": 3, "sequence": 3, "error": "HTTP 529"}),
            json!({"type": "turn_started", "turn_id": 4, "sequence": 4}),
        ];

        let texts = transcript_entries(&messages, &events)
            .into_iter()
            .map(|entry| entry.text)
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            [
                "first",
                "list_files .\nInterrupted before result was saved. Outcome unknown; inspect workspace before retrying.",
                "Request failed: Cancelled",
                "second",
                "Request failed: HTTP 529",
                "third",
            ]
        );
    }

    #[test]
    fn a_call_without_a_saved_result_says_so() {
        let messages = vec![Message::assistant_blocks(vec![tool_use(
            "a",
            "git_status",
            json!({}),
        )])];
        assert_eq!(
            transcript_entries(&messages, &[]),
            [entry(
                TranscriptRole::Tool { failed: true },
                "git_status\nNo result was saved"
            )]
        );
    }
}
