//! Append-only JSONL record of activity events, replayed at startup so the
//! Activity pane survives a restart.

use crate::activity::{ActivityEvent, ActivityKind};
use crate::agent::state::{open_state_append, read_state_bytes};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

pub(crate) const ACTIVITY_LOG_FILE_NAME: &str = "activity.jsonl";
pub(crate) const ROTATED_FILE_NAME: &str = "activity.1.jsonl";
const LOG_KIND: &str = "socium_activity_log";
const LOG_VERSION: u32 = 1;
const ROTATE_BYTES: u64 = 16 * 1024 * 1024;
/// Rotation happens before a write, so the live file can exceed the
/// threshold by one batch.
const MAX_READ_BYTES: u64 = 2 * ROTATE_BYTES;
pub(crate) const REPLAY_TURNS: usize = 20;

#[derive(Serialize, Deserialize)]
struct Header {
    #[serde(rename = "type")]
    kind: String,
    version: u32,
}

/// Collects a turn's events, merging streamed text into one delta per
/// block, and appends them to the log at block and turn boundaries.
pub(crate) struct ActivityRecorder {
    path: PathBuf,
    pending: Vec<ActivityEvent>,
    streams: HashMap<(usize, usize), ActivityEvent>,
    warned: bool,
}

impl ActivityRecorder {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            pending: Vec::new(),
            streams: HashMap::new(),
            warned: false,
        }
    }

    /// Records `event`. Returns a message the first time writing fails;
    /// recording never fails the turn.
    pub(crate) fn observe(&mut self, event: &ActivityEvent) -> Option<String> {
        let key = |block: usize| (event.round, block);
        match &event.kind {
            ActivityKind::ThoughtDelta { block, text } => {
                self.merge(key(*block), event, text, |kind| match kind {
                    ActivityKind::ThoughtDelta { text, .. } => Some(text),
                    _ => None,
                });
                return None;
            }
            ActivityKind::ToolIntentDelta {
                block,
                partial_json,
            } => {
                self.merge(key(*block), event, partial_json, |kind| match kind {
                    ActivityKind::ToolIntentDelta { partial_json, .. } => Some(partial_json),
                    _ => None,
                });
                return None;
            }
            ActivityKind::ThoughtFinished { block, .. }
            | ActivityKind::ToolIntentFinished { block } => {
                if let Some(merged) = self.streams.remove(&key(*block)) {
                    self.pending.push(merged);
                }
            }
            ActivityKind::RoundFinished { .. } | ActivityKind::TurnFinished { .. } => {
                // Streams cut off by an error or cancellation.
                let mut open = self
                    .streams
                    .drain()
                    .filter(|((round, _), _)| {
                        *round == event.round
                            || matches!(event.kind, ActivityKind::TurnFinished { .. })
                    })
                    .collect::<Vec<_>>();
                open.sort_by_key(|(key, _)| *key);
                self.pending
                    .extend(open.into_iter().map(|(_, merged)| merged));
            }
            _ => {}
        }
        self.pending.push(event.clone());
        match self.flush() {
            Ok(()) => None,
            Err(err) if !self.warned => {
                self.warned = true;
                Some(format!("Activity is not being saved: {err}"))
            }
            Err(_) => None,
        }
    }

    fn merge(
        &mut self,
        key: (usize, usize),
        event: &ActivityEvent,
        fragment: &str,
        text_of: impl Fn(&mut ActivityKind) -> Option<&mut String>,
    ) {
        match self.streams.get_mut(&key) {
            Some(merged) => {
                if let Some(text) = text_of(&mut merged.kind) {
                    text.push_str(fragment);
                }
            }
            None => {
                self.streams.insert(key, event.clone());
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut batch = Vec::new();
        for event in &self.pending {
            serde_json::to_writer(&mut batch, event)?;
            batch.push(b'\n');
        }
        // Drop the batch even on failure so a broken log does not grow memory.
        self.pending.clear();
        append(&self.path, &batch)
    }
}

fn append(path: &Path, batch: &[u8]) -> io::Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.len() >= ROTATE_BYTES) {
        rotate(path)?;
    }
    let mut file = open_state_append(path)?;
    if file.metadata()?.len() == 0 {
        let header = Header {
            kind: LOG_KIND.into(),
            version: LOG_VERSION,
        };
        let mut line = serde_json::to_vec(&header)?;
        line.push(b'\n');
        file.write_all(&line)?;
    }
    file.write_all(batch)
}

/// Moves the log aside, keeping one previous generation.
pub(crate) fn rotate(path: &Path) -> io::Result<()> {
    match fs::rename(path, path.with_file_name(ROTATED_FILE_NAME)) {
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Events of the most recent `max_turns` turns. A missing, unreadable, or
/// newer-versioned log yields nothing; malformed lines, such as one cut off
/// by a crash, are skipped.
pub(crate) fn load_recent(path: &Path, max_turns: usize) -> Vec<ActivityEvent> {
    let Ok(Some(bytes)) = read_state_bytes(path, MAX_READ_BYTES) else {
        return Vec::new();
    };
    let mut lines = bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace));
    let Some(Ok(header)) = lines.next().map(serde_json::from_slice::<Header>) else {
        return Vec::new();
    };
    if header.kind != LOG_KIND || header.version != LOG_VERSION {
        return Vec::new();
    }
    let events = lines
        .filter_map(|line| serde_json::from_slice::<ActivityEvent>(line).ok())
        .collect::<Vec<_>>();
    let start = events
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, event)| matches!(event.kind, ActivityKind::TurnStarted))
        .nth(max_turns.saturating_sub(1))
        .map_or(0, |(index, _)| index);
    events.into_iter().skip(start).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::ThoughtRole;

    fn event(turn_id: usize, round: usize, kind: ActivityKind) -> ActivityEvent {
        ActivityEvent {
            turn_id,
            round,
            at_ms: 0,
            kind,
        }
    }

    fn turn(turn_id: usize) -> Vec<ActivityEvent> {
        vec![
            event(turn_id, 0, ActivityKind::TurnStarted),
            event(turn_id, 1, ActivityKind::RoundStarted { model: "m".into() }),
            event(turn_id, 1, ActivityKind::ThoughtStarted { block: 0 }),
            event(
                turn_id,
                1,
                ActivityKind::ThoughtDelta {
                    block: 0,
                    text: "first ".into(),
                },
            ),
            event(
                turn_id,
                1,
                ActivityKind::ThoughtDelta {
                    block: 0,
                    text: "second".into(),
                },
            ),
            event(
                turn_id,
                1,
                ActivityKind::ThoughtFinished {
                    block: 0,
                    role: ThoughtRole::Reasoning,
                    interrupted: false,
                },
            ),
            event(
                turn_id,
                1,
                ActivityKind::ToolFinished {
                    call_id: "c".into(),
                    name: "read_file".into(),
                    is_error: false,
                    elapsed_ms: 3,
                    summary: "Path: a.rs".into(),
                },
            ),
            event(
                turn_id,
                0,
                ActivityKind::TurnFinished {
                    ok: true,
                    error: None,
                },
            ),
        ]
    }

    fn state_dir() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(".socium").join(ACTIVITY_LOG_FILE_NAME);
        (root, path)
    }

    fn record(path: &Path, events: &[ActivityEvent]) {
        let mut recorder = ActivityRecorder::new(path.to_path_buf());
        for event in events {
            assert_eq!(recorder.observe(event), None);
        }
    }

    #[test]
    fn deltas_are_merged_per_block_and_reloaded_in_order() {
        let (_root, path) = state_dir();
        record(&path, &turn(1));

        let loaded = load_recent(&path, REPLAY_TURNS);
        let mut expected = turn(1);
        expected.remove(4);
        if let ActivityKind::ThoughtDelta { text, .. } = &mut expected[3].kind {
            *text = "first second".into();
        }
        assert_eq!(loaded, expected);
    }

    #[test]
    fn only_the_most_recent_turns_are_replayed() {
        let (_root, path) = state_dir();
        for turn_id in 0..5 {
            record(&path, &turn(turn_id));
        }

        let loaded = load_recent(&path, 2);
        assert_eq!(loaded.first().unwrap().turn_id, 3);
        assert!(loaded.iter().all(|event| event.turn_id >= 3));
    }

    #[test]
    fn a_truncated_last_line_and_unknown_lines_are_skipped() {
        let (_root, path) = state_dir();
        record(&path, &turn(1));
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"turn_id\":2,\"round\":0,\"at_ms\":0,\"type\":\"future_kind\"}\n{\"turn_id\":2,\"rou")
            .unwrap();

        let loaded = load_recent(&path, REPLAY_TURNS);
        assert_eq!(loaded.len(), turn(1).len() - 1);
    }

    #[test]
    fn a_newer_log_version_is_not_replayed() {
        let (_root, path) = state_dir();
        record(&path, &turn(1));
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("\"version\":1", "\"version\":9");
        fs::write(&path, text).unwrap();

        assert!(load_recent(&path, REPLAY_TURNS).is_empty());
    }

    #[test]
    fn tool_results_are_recorded_only_as_summaries() {
        let (_root, path) = state_dir();
        record(&path, &turn(1));

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.lines().next().unwrap().contains(LOG_KIND));
        assert!(text.contains("\"summary\":\"Path: a.rs\""));
        assert!(!text.contains("\"content\""));
    }

    #[test]
    fn large_logs_rotate_keeping_one_generation() {
        let (_root, path) = state_dir();
        record(&path, &turn(1));
        let file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.set_len(ROTATE_BYTES).unwrap();
        record(&path, &turn(2));

        let rotated = path.with_file_name(ROTATED_FILE_NAME);
        assert_eq!(fs::metadata(&rotated).unwrap().len(), ROTATE_BYTES);
        let loaded = load_recent(&path, REPLAY_TURNS);
        assert!(loaded.iter().all(|event| event.turn_id == 2));
    }

    #[test]
    fn a_write_failure_warns_once_and_does_not_panic() {
        let root = tempfile::tempdir().unwrap();
        // The log path is a directory, so every append fails.
        let path = root.path().join(".socium").join(ACTIVITY_LOG_FILE_NAME);
        fs::create_dir_all(&path).unwrap();
        let mut recorder = ActivityRecorder::new(path);
        let events = turn(1);

        assert!(recorder.observe(&events[0]).is_some());
        assert!(events[1..].iter().all(|e| recorder.observe(e).is_none()));
    }

    #[test]
    fn streams_cut_off_by_a_failed_round_are_still_recorded() {
        let (_root, path) = state_dir();
        let mut events = turn(1);
        events.truncate(5);
        events.push(event(
            1,
            1,
            ActivityKind::RoundFinished {
                stop_reason: None,
                usage: None,
                ttft_ms: None,
                elapsed_ms: 9,
            },
        ));
        record(&path, &events);

        let loaded = load_recent(&path, REPLAY_TURNS);
        assert!(loaded.iter().any(|e| matches!(
            &e.kind,
            ActivityKind::ThoughtDelta { text, .. } if text == "first second"
        )));
    }
}
