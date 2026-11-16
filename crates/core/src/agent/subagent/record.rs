//! Saved subagent conversations, one file per subagent in
//! `.socium/agents/<id>.json`, so a run can be read back after a restart
//! and continued with `resume`.
//!
//! A record is saved before the subagent's first request and at every tool
//! checkpoint of its loop, so a side effect it starts is on disk first, as
//! for the main conversation.

use crate::agent::state::{load_json_file, save_json_file, workspace_state_path};
use crate::agent::types::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const RECORDS_DIR: &str = "agents";
const MAX_ID_CHARS: usize = 96;
/// Older records beyond this many are removed when a new one is created.
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
    pub(crate) messages: Vec<Message>,
    /// The `tool_started` checkpoint of each call that ran, saying how it
    /// was authorized, so the transcript can mark the approved ones.
    #[serde(default)]
    pub(crate) tool_events: Vec<Value>,
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

    pub(crate) fn save(&mut self, root: &Path) -> io::Result<()> {
        self.updated_at_ms = now_ms();
        save_json_file(&record_path(root, &self.id)?, self)
    }
}

/// A file name for a new subagent, from the call that starts it, unique
/// among the saved ones.
pub(crate) fn new_id(root: &Path, call_id: &str) -> io::Result<String> {
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
    while record_path(root, &id)?.exists() {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    Ok(id)
}

/// The saved subagent `id`, or `None` when there is none. The ID comes from
/// a model, so anything that is not a plain file name is refused.
pub(crate) fn load(root: &Path, id: &str) -> io::Result<Option<SubagentRecord>> {
    if id.is_empty()
        || id.len() > MAX_ID_CHARS + 8
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("'{id}' is not a subagent ID"),
        ));
    }
    let record: Option<SubagentRecord> = load_json_file(&record_path(root, id)?)?;
    Ok(record)
}

/// Saved subagents, most recently updated first. Unreadable files are
/// left out.
pub(crate) fn list(root: &Path) -> io::Result<Vec<SubagentRecord>> {
    let dir = root
        .join(crate::constants::STATE_DIR_NAME)
        .join(RECORDS_DIR);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut records = entries
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            load(root, name.strip_suffix(".json")?).ok().flatten()
        })
        .collect::<Vec<_>>();
    records.sort_by(|a, b| {
        b.updated_at_ms
            .cmp(&a.updated_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(records)
}

/// Removes the least recently updated records beyond `MAX_RECORDS`, never
/// `keep`, the one just created: a clock that went back can date it older
/// than the rest.
pub(crate) fn prune(root: &Path, keep: &str) -> io::Result<()> {
    let others = list(root)?.into_iter().filter(|record| record.id != keep);
    for record in others.skip(MAX_RECORDS - 1) {
        fs::remove_file(record_path(root, &record.id)?)?;
    }
    Ok(())
}

fn record_path(root: &Path, id: &str) -> io::Result<std::path::PathBuf> {
    workspace_state_path(root, &format!("{RECORDS_DIR}/{id}.json"))
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

    #[test]
    fn ids_come_from_the_call_and_stay_plain_unique_file_names() {
        let root = make_temp_dir("record_ids");
        assert_eq!(new_id(root.path(), "toolu_01AB-c").unwrap(), "toolu_01AB-c");
        assert_eq!(new_id(root.path(), "call/../x y").unwrap(), "call____x_y");
        assert_eq!(new_id(root.path(), "").unwrap(), "agent");

        let mut first = SubagentRecord::new("same".into(), "explorer", "d", "m");
        first.save(root.path()).unwrap();
        assert_eq!(new_id(root.path(), "same").unwrap(), "same-2");

        for bad in ["../same", "a/b", "", "with space", ".json"] {
            let err = load(root.path(), bad).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::InvalidInput, "{bad}");
        }
        assert!(load(root.path(), "missing").unwrap().is_none());
        assert_eq!(load(root.path(), "same").unwrap().unwrap().id, "same");
    }

    #[test]
    fn pruning_keeps_the_most_recently_updated_records() {
        let root = make_temp_dir("record_prune");
        for index in 0..=MAX_RECORDS {
            let mut record = SubagentRecord::new(format!("r{index:03}"), "explorer", "d", "m");
            record.updated_at_ms = index as u64;
            save_json_file(&record_path(root.path(), &record.id).unwrap(), &record).unwrap();
        }

        prune(root.path(), &format!("r{MAX_RECORDS:03}")).unwrap();

        let kept = list(root.path()).unwrap();
        assert_eq!(kept.len(), MAX_RECORDS);
        assert_eq!(kept[0].id, format!("r{MAX_RECORDS:03}"));
        assert!(load(root.path(), "r000").unwrap().is_none());
        assert!(load(root.path(), "r001").unwrap().is_some());
    }

    #[test]
    fn a_record_saved_before_tool_events_were_kept_still_loads() {
        let root = make_temp_dir("record_v1");
        let path = record_path(root.path(), "old").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"schema_version":1,"id":"old","agent_type":"explorer","description":"d","model":"m","status":"finished","runs":1,"updated_at_ms":5,"messages":[]}"#,
        )
        .unwrap();

        let record = load(root.path(), "old").unwrap().unwrap();
        assert_eq!(record.id, "old");
        assert!(record.tool_events.is_empty());
    }

    #[test]
    fn pruning_never_removes_the_new_record_even_when_the_clock_went_back() {
        let root = make_temp_dir("record_prune_skew");
        for index in 0..MAX_RECORDS {
            let mut record = SubagentRecord::new(format!("r{index:03}"), "explorer", "d", "m");
            record.updated_at_ms = u64::MAX - index as u64;
            save_json_file(&record_path(root.path(), &record.id).unwrap(), &record).unwrap();
        }
        let mut new = SubagentRecord::new("new".into(), "explorer", "d", "m");
        new.save(root.path()).unwrap();

        prune(root.path(), "new").unwrap();

        assert_eq!(list(root.path()).unwrap().len(), MAX_RECORDS);
        assert!(load(root.path(), "new").unwrap().is_some());
        let oldest = format!("r{:03}", MAX_RECORDS - 1);
        assert!(load(root.path(), &oldest).unwrap().is_none());
    }
}
