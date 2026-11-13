//! State derived from activity by fixed rules: what the agent is doing, and
//! notable patterns such as repeating itself or changing course after a
//! failure. Every signal follows from events shown in the Activity pane.

use super::{ActivityEvent, ActivityKind, ThoughtRole};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const REPEAT_THRESHOLD: usize = 3;
const SILENT_ROUNDS_THRESHOLD: usize = 5;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Thinking,
    Investigating,
    Editing,
    Verifying,
    Committing,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Thinking => "thinking",
            Self::Investigating => "investigating",
            Self::Editing => "editing",
            Self::Verifying => "verifying",
            Self::Committing => "committing",
        }
    }

    fn of_tool(name: &str) -> Self {
        match name {
            "apply_patch" | "str_replace_based_edit_tool" => Self::Editing,
            "run_command" => Self::Verifying,
            "git_commit" => Self::Committing,
            _ => Self::Investigating,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum Signal {
    /// The same tool started with the same input several times in a turn.
    Repeating {
        tool: String,
        input_summary: String,
        count: usize,
    },
    /// A tool failed and the next call tried something different.
    Recovering {
        failed_tool: String,
        next_tool: String,
    },
    /// Several tool rounds in a row ended without a progress update or text.
    Silent { rounds: usize },
}

impl Signal {
    pub fn describe(&self) -> String {
        match self {
            Self::Repeating {
                tool,
                input_summary,
                count,
            } => {
                let target = if input_summary.is_empty() {
                    String::new()
                } else {
                    format!(" {input_summary}")
                };
                format!("Repeating: {tool}{target} started {count} times this turn")
            }
            Self::Recovering {
                failed_tool,
                next_tool,
            } if failed_tool == next_tool => {
                format!("Changing course: retrying {next_tool} with different input")
            }
            Self::Recovering {
                failed_tool,
                next_tool,
            } => format!("Changing course: {failed_tool} failed, trying {next_tool}"),
            Self::Silent { rounds } => {
                format!("{rounds} tool rounds in a row without an update")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalChange {
    Phase(Phase),
    Raised(Signal),
}

#[derive(Default)]
struct TurnState {
    starts: HashMap<(String, String), usize>,
    /// Tool and input summary of the call that is running or just finished.
    last_started: Option<(String, String)>,
    failed: Option<(String, String)>,
    silent_rounds: usize,
    silent_reported: bool,
    round_spoke: bool,
}

/// Folds activity events into the current phase and raised signals.
#[derive(Default)]
pub struct SignalTracker {
    phase: Phase,
    turn: TurnState,
}

impl SignalTracker {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn observe(&mut self, event: &ActivityEvent) -> Vec<SignalChange> {
        let mut changes = Vec::new();
        match &event.kind {
            ActivityKind::TurnStarted => self.turn = TurnState::default(),
            ActivityKind::RoundStarted { .. } => {
                self.turn.round_spoke = false;
                self.set_phase(Phase::Thinking, &mut changes);
            }
            ActivityKind::ThoughtFinished {
                role: ThoughtRole::ProgressUpdate,
                ..
            }
            | ActivityKind::TextFinished { .. } => self.turn.round_spoke = true,
            ActivityKind::RoundFinished { stop_reason, .. } => {
                if stop_reason.as_deref() == Some("tool_use") && !self.turn.round_spoke {
                    self.turn.silent_rounds += 1;
                    if self.turn.silent_rounds >= SILENT_ROUNDS_THRESHOLD
                        && !self.turn.silent_reported
                    {
                        self.turn.silent_reported = true;
                        changes.push(SignalChange::Raised(Signal::Silent {
                            rounds: self.turn.silent_rounds,
                        }));
                    }
                } else {
                    self.turn.silent_rounds = 0;
                }
            }
            ActivityKind::ToolStarted {
                name,
                input_summary,
                ..
            } => {
                self.set_phase(Phase::of_tool(name), &mut changes);
                let key = (name.clone(), input_summary.clone());
                if let Some((failed_tool, failed_input)) = self.turn.failed.take()
                    && (failed_tool != *name || failed_input != *input_summary)
                {
                    changes.push(SignalChange::Raised(Signal::Recovering {
                        failed_tool,
                        next_tool: name.clone(),
                    }));
                }
                self.turn.last_started = Some(key.clone());
                let count = self.turn.starts.entry(key).or_default();
                *count += 1;
                if *count == REPEAT_THRESHOLD {
                    changes.push(SignalChange::Raised(Signal::Repeating {
                        tool: name.clone(),
                        input_summary: input_summary.clone(),
                        count: *count,
                    }));
                }
            }
            ActivityKind::ToolFinished { is_error, .. } => {
                self.turn.failed = if *is_error {
                    self.turn.last_started.clone()
                } else {
                    None
                };
            }
            ActivityKind::TurnFinished { .. } => self.set_phase(Phase::Idle, &mut changes),
            _ => {}
        }
        changes
    }

    fn set_phase(&mut self, phase: Phase, changes: &mut Vec<SignalChange>) {
        if self.phase != phase {
            self.phase = phase;
            changes.push(SignalChange::Phase(phase));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: ActivityKind) -> ActivityEvent {
        ActivityEvent {
            turn_id: 1,
            round: 1,
            at_ms: 0,
            kind,
        }
    }

    fn started(name: &str, input: &str) -> ActivityEvent {
        event(ActivityKind::ToolStarted {
            call_id: "c".into(),
            name: name.into(),
            input_summary: input.into(),
        })
    }

    fn finished(name: &str, is_error: bool) -> ActivityEvent {
        event(ActivityKind::ToolFinished {
            call_id: "c".into(),
            name: name.into(),
            is_error,
            elapsed_ms: 1,
            summary: String::new(),
        })
    }

    fn round(stop: &str) -> [ActivityEvent; 2] {
        [
            event(ActivityKind::RoundStarted { model: "m".into() }),
            event(ActivityKind::RoundFinished {
                stop_reason: Some(stop.into()),
                usage: None,
                ttft_ms: None,
                elapsed_ms: 1,
            }),
        ]
    }

    fn raised(tracker: &mut SignalTracker, events: &[ActivityEvent]) -> Vec<Signal> {
        events
            .iter()
            .flat_map(|e| tracker.observe(e))
            .filter_map(|change| match change {
                SignalChange::Raised(signal) => Some(signal),
                SignalChange::Phase(_) => None,
            })
            .collect()
    }

    #[test]
    fn phase_follows_the_latest_tool_and_returns_to_idle() {
        let mut tracker = SignalTracker::default();
        let phases = [
            event(ActivityKind::TurnStarted),
            event(ActivityKind::RoundStarted { model: "m".into() }),
            started("read_file", "a.rs"),
            started("search_text", "x"),
            started("apply_patch", "a.rs"),
            started("run_command", "cargo test"),
            started("git_commit", "fix"),
            event(ActivityKind::TurnFinished {
                ok: true,
                error: None,
            }),
        ]
        .iter()
        .flat_map(|e| tracker.observe(e))
        .collect::<Vec<_>>();

        assert_eq!(
            phases,
            [
                Phase::Thinking,
                Phase::Investigating,
                Phase::Editing,
                Phase::Verifying,
                Phase::Committing,
                Phase::Idle,
            ]
            .map(SignalChange::Phase)
        );
    }

    #[test]
    fn repeating_is_raised_once_at_the_threshold_and_resets_per_turn() {
        let mut tracker = SignalTracker::default();
        let reads = vec![started("read_file", "a.rs"); 5];

        assert_eq!(
            raised(&mut tracker, &reads),
            [Signal::Repeating {
                tool: "read_file".into(),
                input_summary: "a.rs".into(),
                count: 3
            }]
        );
        tracker.observe(&event(ActivityKind::TurnStarted));
        assert!(raised(&mut tracker, &reads[..2]).is_empty());
    }

    #[test]
    fn a_different_call_after_a_failure_is_recovery_but_a_blind_retry_is_not() {
        let mut tracker = SignalTracker::default();
        let blind_retry = [
            started("read_file", "a.rs"),
            finished("read_file", true),
            started("read_file", "a.rs"),
        ];
        assert!(raised(&mut tracker, &blind_retry).is_empty());

        let mut tracker = SignalTracker::default();
        let recovery = [
            started("apply_patch", "a.rs"),
            finished("apply_patch", true),
            started("str_replace_based_edit_tool", "a.rs"),
            finished("str_replace_based_edit_tool", false),
            started("run_command", "cargo test"),
        ];
        assert_eq!(
            raised(&mut tracker, &recovery),
            [Signal::Recovering {
                failed_tool: "apply_patch".into(),
                next_tool: "str_replace_based_edit_tool".into()
            }]
        );
    }

    #[test]
    fn silence_counts_consecutive_tool_rounds_without_updates() {
        let mut tracker = SignalTracker::default();
        let quiet = (0..6).flat_map(|_| round("tool_use")).collect::<Vec<_>>();
        assert_eq!(raised(&mut tracker, &quiet), [Signal::Silent { rounds: 5 }]);

        let mut tracker = SignalTracker::default();
        let mut spoken = Vec::new();
        for index in 0..6 {
            let [start, finish] = round("tool_use");
            spoken.push(start);
            if index == 3 {
                spoken.push(event(ActivityKind::ThoughtFinished {
                    block: 0,
                    role: ThoughtRole::ProgressUpdate,
                    interrupted: false,
                }));
            }
            spoken.push(finish);
        }
        assert!(raised(&mut tracker, &spoken).is_empty());
    }

    #[test]
    fn signals_have_readable_descriptions() {
        assert_eq!(
            Signal::Recovering {
                failed_tool: "read_file".into(),
                next_tool: "read_file".into()
            }
            .describe(),
            "Changing course: retrying read_file with different input"
        );
        assert_eq!(
            Signal::Silent { rounds: 5 }.describe(),
            "5 tool rounds in a row without an update"
        );
    }
}
