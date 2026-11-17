//! Structured record of what the model and the agent did during a turn,
//! for the Activity pane, headless output, and replay.

use crate::providers::{BlockKind, ProviderEvent, TokenUsage};
use crate::tools::git::DIRTY_WORKTREE_WARNING;

mod signals;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use signals::{Phase, Signal, SignalChange, SignalTracker};
use std::time::Instant;

/// Progress-update text the API substitutes for work cut off by a stop.
const INTERRUPTED_SENTINEL: &str = "This part of the response was interrupted before it finished.";
const MAX_SUMMARY_CHARS: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub turn_id: usize,
    /// Model request round within the turn, starting at 1. Zero for events
    /// outside the tool loop, such as compaction.
    pub round: usize,
    /// Milliseconds since the turn started.
    pub at_ms: u64,
    /// Call ID of the `agent` tool call whose subagent produced the event;
    /// `None` for the main agent. Subagent rounds count from 1 on their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(flatten)]
    pub kind: ActivityKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThoughtRole {
    /// A summary of the model's reasoning.
    Reasoning,
    /// A short note on what was found and what comes next, written right
    /// before a tool call.
    ProgressUpdate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActivityKind {
    TurnStarted,
    RoundStarted {
        model: String,
    },
    ThoughtStarted {
        block: usize,
    },
    ThoughtDelta {
        block: usize,
        text: String,
    },
    /// Classified once the following block is known: a thought directly
    /// before a tool call is a progress update.
    ThoughtFinished {
        block: usize,
        role: ThoughtRole,
        interrupted: bool,
    },
    TextFinished {
        block: usize,
        chars: usize,
    },
    ToolIntentStarted {
        block: usize,
        call_id: String,
        name: String,
    },
    /// Display-only fragment of the tool input; never executed.
    ToolIntentDelta {
        block: usize,
        partial_json: String,
    },
    ToolIntentFinished {
        block: usize,
    },
    RoundFinished {
        stop_reason: Option<String>,
        usage: Option<TokenUsage>,
        ttft_ms: Option<u64>,
        elapsed_ms: u64,
    },
    ToolStarted {
        call_id: String,
        name: String,
        input_summary: String,
    },
    ApprovalWaiting {
        call_id: String,
        tool: String,
    },
    ApprovalResolved {
        call_id: String,
        approved: bool,
        waited_ms: u64,
    },
    CommandOutput {
        call_id: String,
        stdout_bytes: u64,
        stderr_bytes: u64,
    },
    ToolFinished {
        call_id: String,
        name: String,
        is_error: bool,
        elapsed_ms: u64,
        summary: String,
        /// Part of the result was left out: a command stream beyond its
        /// limit (kept in full in its artifact) or a result beyond
        /// `MAX_TOOL_RESULT_CHARACTERS`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    CompactionStarted,
    CompactionFinished {
        covered_messages: usize,
        used_llm: bool,
    },
    TurnFinished {
        ok: bool,
        error: Option<String>,
    },
    /// A problem with Socium's own bookkeeping, such as saving activity.
    Warning {
        message: String,
    },
    /// The plan after an `update_plan` call, sent once its result is saved.
    /// The Timeline shows it; Activity already shows the call itself.
    PlanUpdated {
        plan: crate::plan::PlanView,
    },
    /// The main agent ran `rounds` tool rounds without updating its open
    /// plan, and a reminder was appended to the round's last result.
    PlanNudged {
        rounds: usize,
    },
}

/// Stamps events with the turn they belong to and the time since it began.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ActivityClock {
    turn_id: usize,
    started: Instant,
}

impl ActivityClock {
    pub(crate) fn start(turn_id: usize) -> Self {
        Self {
            turn_id,
            started: Instant::now(),
        }
    }

    pub(crate) fn turn_id(&self) -> usize {
        self.turn_id
    }

    pub(crate) fn elapsed_ms(&self) -> u64 {
        millis_since(self.started)
    }

    pub(crate) fn event(&self, round: usize, kind: ActivityKind) -> ActivityEvent {
        ActivityEvent {
            turn_id: self.turn_id,
            round,
            at_ms: self.elapsed_ms(),
            agent: None,
            kind,
        }
    }
}

fn millis_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

struct PendingThought {
    block: usize,
    text: String,
    started: bool,
}

/// Turns one model response's provider events into activity, holding back
/// each thought's classification until the next block shows what it led to.
pub(crate) struct RoundActivity {
    started: Instant,
    first_token_ms: Option<u64>,
    thought: Option<PendingThought>,
    text_chars: Option<(usize, usize)>,
    open_tool: Option<usize>,
    finished: bool,
}

impl RoundActivity {
    pub(crate) fn start() -> Self {
        Self {
            started: Instant::now(),
            first_token_ms: None,
            thought: None,
            text_chars: None,
            open_tool: None,
            finished: false,
        }
    }

    pub(crate) fn apply(&mut self, event: ProviderEvent) -> Vec<ActivityKind> {
        let mut out = Vec::new();
        match event {
            ProviderEvent::BlockStarted { index, kind } => {
                let next_is_tool = matches!(kind, BlockKind::ToolUse { .. });
                self.finish_thought(next_is_tool, &mut out);
                match kind {
                    BlockKind::Thinking => {
                        self.thought = Some(PendingThought {
                            block: index,
                            text: String::new(),
                            started: false,
                        });
                    }
                    BlockKind::Text => self.text_chars = Some((index, 0)),
                    BlockKind::ToolUse { id, name } => {
                        self.mark_first_token();
                        self.open_tool = Some(index);
                        out.push(ActivityKind::ToolIntentStarted {
                            block: index,
                            call_id: id,
                            name,
                        });
                    }
                }
            }
            ProviderEvent::ThinkingDelta { index, text } => {
                if text.is_empty() {
                    return out;
                }
                self.mark_first_token();
                if let Some(thought) = self.thought.as_mut().filter(|t| t.block == index) {
                    if !thought.started {
                        thought.started = true;
                        out.push(ActivityKind::ThoughtStarted { block: index });
                    }
                    thought.text.push_str(&text);
                    out.push(ActivityKind::ThoughtDelta { block: index, text });
                }
            }
            ProviderEvent::TextDelta { index, text } => {
                self.mark_first_token();
                if let Some((block, chars)) = self.text_chars.as_mut()
                    && *block == index
                {
                    *chars += text.chars().count();
                }
            }
            ProviderEvent::ToolInputDelta {
                index,
                partial_json,
            } => {
                if !partial_json.is_empty() {
                    out.push(ActivityKind::ToolIntentDelta {
                        block: index,
                        partial_json,
                    });
                }
            }
            ProviderEvent::BlockFinished { index } => {
                if let Some((block, chars)) = self.text_chars.take() {
                    if block == index {
                        out.push(ActivityKind::TextFinished { block, chars });
                    } else {
                        self.text_chars = Some((block, chars));
                    }
                }
                if self.open_tool == Some(index) {
                    self.open_tool = None;
                    out.push(ActivityKind::ToolIntentFinished { block: index });
                }
            }
            ProviderEvent::MessageFinished { stop_reason, usage } => {
                out.extend(self.finish(Some(stop_reason), usage));
            }
            ProviderEvent::Raw { .. } => {}
        }
        out
    }

    /// Closes the round. Without a `MessageFinished` (a failed or cancelled
    /// request) the stop reason and usage are unknown.
    pub(crate) fn finish(
        &mut self,
        stop_reason: Option<String>,
        usage: Option<TokenUsage>,
    ) -> Vec<ActivityKind> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut out = Vec::new();
        self.finish_thought(false, &mut out);
        out.push(ActivityKind::RoundFinished {
            stop_reason,
            usage,
            ttft_ms: self.first_token_ms,
            elapsed_ms: millis_since(self.started),
        });
        out
    }

    fn mark_first_token(&mut self) {
        if self.first_token_ms.is_none() {
            self.first_token_ms = Some(millis_since(self.started));
        }
    }

    fn finish_thought(&mut self, next_is_tool: bool, out: &mut Vec<ActivityKind>) {
        let Some(thought) = self.thought.take() else {
            return;
        };
        if !thought.started {
            return;
        }
        let interrupted = thought.text.trim() == INTERRUPTED_SENTINEL;
        out.push(ActivityKind::ThoughtFinished {
            block: thought.block,
            role: if next_is_tool || interrupted {
                ThoughtRole::ProgressUpdate
            } else {
                ThoughtRole::Reasoning
            },
            interrupted,
        });
    }
}

/// The input field that best says what a tool call is about.
fn primary_input_key(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "read_file" | "list_files" | "git_status" | "git_diff" | "str_replace_based_edit_tool" => {
            "path"
        }
        "search_text" => "pattern",
        "find_symbol" => "symbol",
        "run_command" => "command",
        "apply_patch" => "patch",
        "agent" => "description",
        "git_commit" => "message",
        "update_plan" => "goal",
        _ => return None,
    })
}

/// One-line description of a complete tool input.
pub fn summarize_tool_input(tool: &str, input: &Value) -> String {
    if tool == crate::constants::PLAN_TOOL_NAME {
        return crate::plan::summarize_update(input);
    }
    let Some(key) = primary_input_key(tool) else {
        return String::new();
    };
    match &input[key] {
        Value::String(value) if key == "patch" => patch_targets(value),
        Value::String(value) => one_line(value),
        Value::Array(parts) => one_line(
            &parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => String::new(),
    }
}

/// Best-effort description of a tool input that is still streaming. Reads
/// the primary string field up to where the JSON currently ends.
pub fn summarize_partial_tool_input(tool: &str, partial_json: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<Value>(partial_json) {
        return Some(summarize_tool_input(tool, &value)).filter(|s| !s.is_empty());
    }
    let key = primary_input_key(tool)?;
    let start = partial_json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = partial_json[start..].trim_start().strip_prefix(':')?;
    let rest = rest.trim_start().strip_prefix('"')?;
    let mut value = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => value.push('\n'),
                Some('t') => value.push('\t'),
                Some(other) => value.push(other),
                None => break,
            },
            c => value.push(c),
        }
    }
    let summary = if key == "patch" {
        patch_targets(&value)
    } else {
        one_line(&value)
    };
    Some(summary).filter(|s| !s.is_empty())
}

/// Files named by `*** Update/Add/Delete File:` headers, else the first line.
fn patch_targets(patch: &str) -> String {
    let files = patch
        .lines()
        .filter_map(|line| {
            ["*** Update File:", "*** Add File:", "*** Delete File:"]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(str::trim)
        .collect::<Vec<_>>();
    if files.is_empty() {
        one_line(patch.lines().next().unwrap_or(""))
    } else {
        one_line(&files.join(", "))
    }
}

/// First non-empty line of a tool result, bounded and stripped of control
/// characters so it is safe to print on a terminal.
pub fn summarize_tool_result(content: &str) -> String {
    let mut lines = content.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().unwrap_or("");
    // The dirty-worktree warning prefixes every edit result; show what the
    // tool did instead.
    let line = if first == DIRTY_WORKTREE_WARNING {
        lines.next().unwrap_or(first)
    } else {
        first
    };
    one_line(line)
}

fn one_line(text: &str) -> String {
    let mut chars = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .skip_while(|c| c.is_whitespace());
    let mut shown = chars.by_ref().take(MAX_SUMMARY_CHARS).collect::<String>();
    if chars.next().is_some() {
        shown.push('…');
    }
    shown.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(events: Vec<ProviderEvent>) -> Vec<ActivityKind> {
        let mut round = RoundActivity::start();
        let mut out = events
            .into_iter()
            .flat_map(|e| round.apply(e))
            .collect::<Vec<_>>();
        out.extend(round.finish(None, None));
        out.into_iter()
            .map(|kind| match kind {
                ActivityKind::RoundFinished {
                    stop_reason, usage, ..
                } => ActivityKind::RoundFinished {
                    stop_reason,
                    usage,
                    ttft_ms: None,
                    elapsed_ms: 0,
                },
                other => other,
            })
            .collect()
    }

    fn thinking(index: usize, text: &str) -> [ProviderEvent; 3] {
        [
            ProviderEvent::BlockStarted {
                index,
                kind: BlockKind::Thinking,
            },
            ProviderEvent::ThinkingDelta {
                index,
                text: text.into(),
            },
            ProviderEvent::BlockFinished { index },
        ]
    }

    fn finished(reason: &str) -> ProviderEvent {
        ProviderEvent::MessageFinished {
            stop_reason: reason.into(),
            usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 4,
                ..TokenUsage::default()
            }),
        }
    }

    #[test]
    fn thought_before_a_tool_call_is_a_progress_update() {
        let mut events = Vec::from(thinking(0, "Read the view next."));
        events.extend([
            ProviderEvent::BlockStarted {
                index: 1,
                kind: BlockKind::ToolUse {
                    id: "t1".into(),
                    name: "read_file".into(),
                },
            },
            ProviderEvent::ToolInputDelta {
                index: 1,
                partial_json: "{\"path\":\"src/".into(),
            },
            ProviderEvent::BlockFinished { index: 1 },
            finished("tool_use"),
        ]);

        let out = run(events);

        assert_eq!(
            out,
            vec![
                ActivityKind::ThoughtStarted { block: 0 },
                ActivityKind::ThoughtDelta {
                    block: 0,
                    text: "Read the view next.".into()
                },
                ActivityKind::ThoughtFinished {
                    block: 0,
                    role: ThoughtRole::ProgressUpdate,
                    interrupted: false
                },
                ActivityKind::ToolIntentStarted {
                    block: 1,
                    call_id: "t1".into(),
                    name: "read_file".into()
                },
                ActivityKind::ToolIntentDelta {
                    block: 1,
                    partial_json: "{\"path\":\"src/".into()
                },
                ActivityKind::ToolIntentFinished { block: 1 },
                ActivityKind::RoundFinished {
                    stop_reason: Some("tool_use".into()),
                    usage: Some(TokenUsage {
                        input_tokens: 10,
                        output_tokens: 4,
                        ..TokenUsage::default()
                    }),
                    ttft_ms: None,
                    elapsed_ms: 0
                },
            ]
        );
    }

    #[test]
    fn thought_before_text_is_reasoning_and_text_is_counted() {
        let mut events = Vec::from(thinking(0, "The user wants a summary."));
        events.extend([
            ProviderEvent::BlockStarted {
                index: 1,
                kind: BlockKind::Text,
            },
            ProviderEvent::TextDelta {
                index: 1,
                text: "答えは".into(),
            },
            ProviderEvent::TextDelta {
                index: 1,
                text: "42".into(),
            },
            ProviderEvent::BlockFinished { index: 1 },
            finished("end_turn"),
        ]);

        let out = run(events);

        assert!(out.contains(&ActivityKind::ThoughtFinished {
            block: 0,
            role: ThoughtRole::Reasoning,
            interrupted: false
        }));
        assert!(out.contains(&ActivityKind::TextFinished { block: 1, chars: 5 }));
        assert!(
            !out.iter()
                .any(|k| matches!(k, ActivityKind::ToolIntentFinished { .. }))
        );
    }

    #[test]
    fn empty_thinking_from_an_omitted_display_produces_no_thought() {
        let mut events = Vec::from(thinking(0, ""));
        events.push(finished("end_turn"));

        let out = run(events);

        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], ActivityKind::RoundFinished { .. }));
    }

    #[test]
    fn interrupted_sentinel_is_flagged() {
        let mut events = Vec::from(thinking(0, INTERRUPTED_SENTINEL));
        events.push(finished("max_tokens"));

        let out = run(events);

        assert!(out.contains(&ActivityKind::ThoughtFinished {
            block: 0,
            role: ThoughtRole::ProgressUpdate,
            interrupted: true
        }));
    }

    #[test]
    fn a_failed_request_still_closes_the_round_once_without_usage() {
        let mut round = RoundActivity::start();
        for event in thinking(0, "partial") {
            round.apply(event);
        }

        let out = round.finish(None, None);
        assert!(matches!(
            out.as_slice(),
            [
                ActivityKind::ThoughtFinished {
                    role: ThoughtRole::Reasoning,
                    ..
                },
                ActivityKind::RoundFinished {
                    stop_reason: None,
                    usage: None,
                    ..
                }
            ]
        ));
        assert!(round.finish(None, None).is_empty());
    }

    #[test]
    fn events_serialize_with_a_flat_type_tag() {
        let event = ActivityClock::start(3).event(
            2,
            ActivityKind::ToolFinished {
                call_id: "t1".into(),
                name: "run_command".into(),
                is_error: true,
                elapsed_ms: 12,
                summary: "exit 1".into(),
                truncated: false,
            },
        );
        let value = serde_json::to_value(&event).unwrap();

        assert_eq!(value["type"], "tool_finished");
        assert_eq!(value["turn_id"], 3);
        assert_eq!(value["round"], 2);
        assert_eq!(
            serde_json::from_value::<ActivityEvent>(value).unwrap(),
            event
        );
    }

    #[test]
    fn a_saved_tool_event_without_the_cut_flag_still_loads() {
        let saved = r#"{"turn_id":1,"round":1,"at_ms":5,"type":"tool_finished","call_id":"t","name":"read_file","is_error":false,"elapsed_ms":3,"summary":"ok"}"#;
        let event: ActivityEvent = serde_json::from_str(saved).unwrap();
        assert!(matches!(
            event.kind,
            ActivityKind::ToolFinished {
                truncated: false,
                ..
            }
        ));
        assert!(!serde_json::to_string(&event).unwrap().contains("truncated"));
    }

    #[test]
    fn tool_inputs_are_summarized_by_their_primary_field() {
        assert_eq!(
            summarize_tool_input("run_command", &json!({"command": ["cargo", "test"]})),
            "cargo test"
        );
        assert_eq!(
            summarize_tool_input("read_file", &json!({"path": "src/main.rs"})),
            "src/main.rs"
        );
        assert_eq!(
            summarize_tool_input(
                "apply_patch",
                &json!({"patch": "*** Begin Patch\n*** Update File: a.rs\n*** Add File: b.rs\n"})
            ),
            "a.rs, b.rs"
        );
        assert_eq!(summarize_tool_input("git_branch", &json!({})), "");
    }

    #[test]
    fn partial_tool_inputs_show_the_primary_field_so_far() {
        assert_eq!(
            summarize_partial_tool_input("search_text", "{\"pattern\": \"push_act"),
            Some("push_act".into())
        );
        assert_eq!(
            summarize_partial_tool_input("read_file", "{\"path\":\"a\\\"b.rs\"}"),
            Some("a\"b.rs".into())
        );
        assert_eq!(summarize_partial_tool_input("read_file", "{\"pa"), None);
        assert_eq!(summarize_partial_tool_input("git_branch", "{"), None);
    }

    #[test]
    fn summaries_are_bounded_single_lines_without_control_characters() {
        let summary =
            summarize_tool_result(&format!("\n\n\u{1b}[31merror{}\nnext", "x".repeat(300)));
        assert!(summary.starts_with("[31merror"));
        assert!(summary.ends_with('…'));
        assert!(!summary.chars().any(char::is_control));
        assert_eq!(summary.chars().count(), MAX_SUMMARY_CHARS + 1);
    }

    #[test]
    fn result_summaries_skip_the_dirty_worktree_warning() {
        let content = format!("{DIRTY_WORKTREE_WARNING}\nCreated file: notes.md");
        assert_eq!(summarize_tool_result(&content), "Created file: notes.md");
        assert_eq!(
            summarize_tool_result(DIRTY_WORKTREE_WARNING),
            DIRTY_WORKTREE_WARNING
        );
    }
}
