//! Saved subagent conversations, one file per subagent in
//! `.socium/sessions/<session>/agents/<id>.json`, so a run can be read back
//! after a restart and continued with `resume`. A record lives in the
//! session that started it, so listing and resuming see only that
//! session's subagents, and deleting the session deletes them.
//!
//! A record is saved before the subagent's first request and at every tool
//! checkpoint of its loop, so a side effect it starts is on disk first, as
//! for the main conversation.

use crate::constants::STATE_DIR_NAME;
use crate::conversation::Message;
use crate::session::{SESSIONS_DIR_NAME, session_file};
use crate::state::{
    ConversationSummaryState, check_state_file, load_json_file, save_json_file,
    workspace_state_path,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const RECORDS_DIR: &str = "agents";
const MAX_ID_CHARS: usize = 96;
/// A session's older records beyond this many are removed when a new one
/// is created.
pub(crate) const MAX_RECORDS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    /// Running, or stopped by a crash before it could say otherwise.
    Running,
    Finished,
    Failed,
    Cancelled,
}

impl SubagentStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running or interrupted",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SubagentRecord {
    pub(crate) schema_version: u32,
    pub(crate) id: String,
    pub(crate) agent_type: String,
    /// The task label of the call that started it.
    pub(crate) description: String,
    /// Model ID of its latest run; thinking in `messages` is bound to it.
    pub(crate) model: String,
    pub(crate) status: SubagentStatus,
    /// Prompts it was given: the first task and each resume.
    pub(crate) runs: usize,
    pub(crate) updated_at_ms: u64,
    /// Every message, kept whole after compaction as in the main
    /// conversation.
    pub(crate) messages: Vec<Message>,
    /// The `tool_started` checkpoint of each call that ran, saying how it
    /// was authorized, so the transcript can mark the approved ones.
    #[serde(default)]
    pub(crate) tool_events: Vec<Value>,
    /// What replaces the older `messages` when they are sent.
    #[serde(default)]
    pub(crate) summary_state: ConversationSummaryState,
}

/// What `/agents` lists about a saved subagent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentSummary {
    pub id: String,
    pub agent_type: String,
    pub description: String,
    pub model: String,
    pub status: SubagentStatus,
    pub runs: usize,
    /// Milliseconds since the Unix epoch.
    pub updated_at_ms: u64,
}

impl SubagentRecord {
    pub(crate) fn new(id: String, agent_type: &str, description: &str, model: &str) -> Self {
        Self {
            schema_version: 1,
            id,
            agent_type: agent_type.to_string(),
            description: description.to_string(),
            model: model.to_string(),
            status: SubagentStatus::Running,
            runs: 0,
            updated_at_ms: now_ms(),
            messages: Vec::new(),
            tool_events: Vec::new(),
            summary_state: ConversationSummaryState::default(),
        }
    }

    pub(crate) fn summary(&self) -> SubagentSummary {
        SubagentSummary {
            id: self.id.clone(),
            agent_type: self.agent_type.clone(),
            description: self.description.clone(),
            model: self.model.clone(),
            status: self.status,
            runs: self.runs,
            updated_at_ms: self.updated_at_ms,
        }
    }

    pub(crate) fn save(&mut self, root: &Path, session: &str) -> io::Result<()> {
        self.updated_at_ms = now_ms();
        save_json_file(&record_path(root, session, &self.id)?, self)
    }
}

/// A file name for a new subagent, from the call that starts it, unique
/// among the session's saved ones.
pub(crate) fn new_id(root: &Path, session: &str, call_id: &str) -> io::Result<String> {
    let base = call_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_ID_CHARS)
        .collect::<String>();
    let base = if base.is_empty() {
        "agent".to_string()
    } else {
        base
    };
    let mut id = base.clone();
    let mut suffix = 2;
    while record_path(root, session, &id)?.exists() {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    Ok(id)
}

fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_CHARS + 8
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The subagent `id` saved in `session`, or `None` when there is none. The
/// ID comes from a model, so anything that is not a plain file name is
/// refused.
pub(crate) fn load(root: &Path, session: &str, id: &str) -> io::Result<Option<SubagentRecord>> {
    if !is_id(id) {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("'{id}' is not a subagent ID"),
        ));
    }
    let record: Option<SubagentRecord> = load_json_file(&record_path(root, session, id)?)?;
    Ok(record)
}

/// IDs of the record files in `dir`.
fn record_ids(dir: &Path) -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    Ok(entries
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            Some(name.strip_suffix(".json")?.to_string())
        })
        .filter(|id| is_id(id))
        .collect())
}

/// The subagents saved in `session`, most recently updated first.
/// Unreadable files are left out.
pub(crate) fn list(root: &Path, session: &str) -> io::Result<Vec<SubagentRecord>> {
    let mut records = record_ids(&session_file(root, session, RECORDS_DIR))?
        .iter()
        .filter_map(|id| load(root, session, id).ok().flatten())
        .collect::<Vec<_>>();
    records.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(records)
}

/// Removes the session's least recently updated records beyond
/// `MAX_RECORDS`, never `keep`, the one just created: a clock that went
/// back can date it older than the rest.
pub(crate) fn prune(root: &Path, session: &str, keep: &str) -> io::Result<()> {
    let others = list(root, session)?
        .into_iter()
        .filter(|record| record.id != keep);
    for record in others.skip(MAX_RECORDS - 1) {
        fs::remove_file(record_path(root, session, &record.id)?)?;
    }
    Ok(())
}

/// Moves the records saved before records belonged to a session, from
/// `.socium/agents/`, into `session`. A record whose ID the session already
/// has stays where it was, so nothing is overwritten. Call with the
/// workspace lock held.
pub(crate) fn adopt_legacy_records(root: &Path, session: &str) -> io::Result<()> {
    let legacy = root.join(STATE_DIR_NAME).join(RECORDS_DIR);
    for id in record_ids(&legacy)? {
        let from = legacy.join(format!("{id}.json"));
        check_state_file(&fs::symlink_metadata(&from)?)?;
        let to = record_path(root, session, &id)?;
        if fs::symlink_metadata(&to).is_err() {
            fs::rename(&from, &to)?;
        }
    }
    // Fails, and keeps the directory, while a record stayed behind.
    let _ = fs::remove_dir(&legacy);
    Ok(())
}

fn record_path(root: &Path, session: &str, id: &str) -> io::Result<PathBuf> {
    workspace_state_path(
        root,
        &format!("{SESSIONS_DIR_NAME}/{session}/{RECORDS_DIR}/{id}.json"),
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;

    const SESSION: &str = "20261101-000000-0000";
    const OTHER: &str = "20261102-000000-0000";

    #[test]
    fn ids_come_from_the_call_and_stay_plain_unique_file_names() {
        let root = make_temp_dir("record_ids");
        let root = root.path();
        assert_eq!(
            new_id(root, SESSION, "toolu_01AB-c").unwrap(),
            "toolu_01AB-c"
        );
        assert_eq!(new_id(root, SESSION, "call/../x y").unwrap(), "call____x_y");
        assert_eq!(new_id(root, SESSION, "").unwrap(), "agent");

        let mut first = SubagentRecord::new("same".into(), "explorer", "d", "m");
        first.save(root, SESSION).unwrap();
        assert_eq!(new_id(root, SESSION, "same").unwrap(), "same-2");

        for bad in ["../same", "a/b", "", "with space", ".json"] {
            let err = load(root, SESSION, bad).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::InvalidInput, "{bad}");
        }
        assert!(load(root, SESSION, "missing").unwrap().is_none());
        assert_eq!(load(root, SESSION, "same").unwrap().unwrap().id, "same");
        assert!(load(root, OTHER, "same").unwrap().is_none());
    }

    #[test]
    fn pruning_keeps_the_most_recently_updated_records() {
        let root = make_temp_dir("record_prune");
        for index in 0..=MAX_RECORDS {
            let mut record = SubagentRecord::new(format!("r{index:03}"), "explorer", "d", "m");
            record.updated_at_ms = index as u64;
            save_json_file(
                &record_path(root.path(), SESSION, &record.id).unwrap(),
                &record,
            )
            .unwrap();
        }

        prune(root.path(), SESSION, &format!("r{MAX_RECORDS:03}")).unwrap();

        let kept = list(root.path(), SESSION).unwrap();
        assert_eq!(kept.len(), MAX_RECORDS);
        assert_eq!(kept[0].id, format!("r{MAX_RECORDS:03}"));
        assert!(load(root.path(), SESSION, "r000").unwrap().is_none());
        assert!(load(root.path(), SESSION, "r001").unwrap().is_some());
    }

    #[test]
    fn a_record_saved_before_tool_events_were_kept_still_loads() {
        let root = make_temp_dir("record_v1");
        let path = record_path(root.path(), SESSION, "old").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"schema_version":1,"id":"old","agent_type":"explorer","description":"d","model":"m","status":"finished","runs":1,"updated_at_ms":5,"messages":[]}"#,
        )
        .unwrap();

        let record = load(root.path(), SESSION, "old").unwrap().unwrap();
        assert_eq!(record.id, "old");
        assert!(record.tool_events.is_empty());
    }

    #[test]
    fn pruning_never_removes_the_new_record_even_when_the_clock_went_back() {
        let root = make_temp_dir("record_prune_skew");
        for index in 0..MAX_RECORDS {
            let mut record = SubagentRecord::new(format!("r{index:03}"), "explorer", "d", "m");
            record.updated_at_ms = u64::MAX - index as u64;
            save_json_file(
                &record_path(root.path(), SESSION, &record.id).unwrap(),
                &record,
            )
            .unwrap();
        }
        let mut new = SubagentRecord::new("new".into(), "explorer", "d", "m");
        new.save(root.path(), SESSION).unwrap();

        prune(root.path(), SESSION, "new").unwrap();

        assert_eq!(list(root.path(), SESSION).unwrap().len(), MAX_RECORDS);
        assert!(load(root.path(), SESSION, "new").unwrap().is_some());
        let oldest = format!("r{:03}", MAX_RECORDS - 1);
        assert!(load(root.path(), SESSION, &oldest).unwrap().is_none());
    }
}
