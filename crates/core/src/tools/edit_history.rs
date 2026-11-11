use crate::agent::state::{atomic_write, load_json_file, save_json_file};
use crate::tools::workspace::{resolve_workspace_mutation_path, resolve_workspace_path_for_create};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Mutation {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
struct EditRecord {
    changes: Vec<Mutation>,
    complete: bool,
}
#[derive(Default, Serialize, Deserialize)]
struct EditHistoryState {
    #[serde(default)]
    schema_version: u32,
    #[serde(flatten)]
    legacy: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    records: Vec<EditRecord>,
}
fn history_path(root: &Path) -> Result<PathBuf, String> {
    resolve_workspace_path_for_create(root, ".socium/edit_history.json").map_err(|e| e.to_string())
}
fn current(path: &Path) -> Result<Option<String>, String> {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(format!("Not a regular file: {}", path.display()));
            }
            crate::tools::text_file::read_utf8_file_with_limit(
                path,
                &path.to_string_lossy(),
                crate::constants::MAX_READ_FILE_BYTES,
            )
            .map(Some)
            .map_err(|e| e.to_string())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Cannot inspect '{}': {e}", path.display())),
    }
}

fn check(root: &Path, change: &Mutation, expected: &Option<String>) -> Result<PathBuf, String> {
    let path = resolve_workspace_mutation_path(root, &change.path).map_err(|e| e.to_string())?;
    if current(&path)? != *expected {
        return Err(format!(
            "File changed externally: '{}'; operation refused",
            change.path
        ));
    }
    Ok(path)
}
fn write(path: &Path, content: &Option<String>) -> Result<(), String> {
    match content {
        Some(text) => atomic_write(path, text.as_bytes()),
        None => std::fs::remove_file(path),
    }
    .map_err(|e| format!("Failed to change '{}': {e}", path.display()))
}

/// All semantic validation precedes the first write. The history entry is
/// durable before mutation; incomplete groups require inspection, never blind
/// rollback. Each file replacement is atomic; a group of files is not.
pub(crate) fn apply_changes(root: &Path, mut changes: Vec<Mutation>) -> Result<(), String> {
    let canonical = root.canonicalize().map_err(|e| e.to_string())?;
    let mut seen: Vec<PathBuf> = Vec::new();
    for change in &mut changes {
        if change
            .after
            .as_ref()
            .is_some_and(|s| s.len() > crate::constants::MAX_READ_FILE_BYTES)
        {
            return Err("Edited file exceeds supported file size limit".into());
        }
        let resolved = check(root, change, &change.before)?;
        if seen
            .iter()
            .any(|p| p.starts_with(&resolved) || resolved.starts_with(p))
        {
            return Err("Overlapping mutation paths; split the patch".into());
        }
        change.path = resolved
            .strip_prefix(&canonical)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
        seen.push(resolved);
    }
    changes.sort_by_key(|change| change.after.is_none());
    let path = history_path(root)?;
    let mut history: EditHistoryState = load_json_file(&path).map_err(|e| e.to_string())?;
    if history.schema_version > 1 {
        return Err("Unsupported edit history schema; original preserved".into());
    }
    history.schema_version = 1;
    history.records.push(EditRecord {
        changes: changes.clone(),
        complete: false,
    });
    save_json_file(&path, &history).map_err(|e| e.to_string())?;
    let mut applied = Vec::new();
    for change in &changes {
        let result =
            check(root, change, &change.before).and_then(|path| write(&path, &change.after));
        if let Err(error) = result {
            return Err(format!(
                "{error}. Applied paths: {applied:?}. Inspect incomplete edit record at {} before recovery.",
                path.display()
            ));
        }
        applied.push(change.path.clone());
    }
    history.records.last_mut().unwrap().complete = true;
    save_json_file(&path,&history).map_err(|e|format!("Files changed ({applied:?}), but history completion could not be saved: {e}; inspect before retrying"))
}

pub(crate) fn undo(root: &Path, requested: &str) -> Result<(), String> {
    let target = resolve_workspace_mutation_path(root, requested).map_err(|e| e.to_string())?;
    let path = history_path(root)?;
    let mut history: EditHistoryState = load_json_file(&path).map_err(|e| e.to_string())?;
    if history.schema_version > 1 {
        return Err("Unsupported edit history schema; original preserved".into());
    }
    history.schema_version = 1;
    let index = history
        .records
        .iter()
        .rposition(|r| {
            r.changes.iter().any(|c| {
                resolve_workspace_path_for_create(root, &c.path).is_ok_and(|p| p == target)
            })
        })
        .ok_or_else(|| {
            format!(
                "No edit history found for '{requested}' (legacy history cannot be safely undone)"
            )
        })?;
    let record = &history.records[index];
    if !record.complete {
        return Err(
            "Incomplete edit or undo; inspect workspace and history before recovery".into(),
        );
    }
    for change in &record.changes {
        check(root, change, &change.after)?;
    }
    let changes = record.changes.clone();
    history.records[index].complete = false;
    save_json_file(&path, &history).map_err(|e| e.to_string())?;
    let mut restored = Vec::new();
    for change in changes.iter().rev() {
        let result = check(root, change, &change.after).and_then(|p| write(&p, &change.before));
        if let Err(e) = result {
            return Err(format!(
                "{e}; restored: {restored:?}. Incomplete undo record retained"
            ));
        }
        restored.push(change.path.clone());
    }
    history.records.remove(index);
    save_json_file(&path, &history)
        .map_err(|e| format!("Undo applied; could not save history: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn external_edits_prevent_undo_without_consuming_history() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("f"), "before").unwrap();
        apply_changes(
            root.path(),
            vec![Mutation {
                path: "f".into(),
                before: Some("before".into()),
                after: Some("after".into()),
            }],
        )
        .unwrap();
        std::fs::write(root.path().join("f"), "human").unwrap();
        assert!(undo(root.path(), "f").is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join("f")).unwrap(),
            "human"
        );
        std::fs::write(root.path().join("f"), "after").unwrap();
        undo(root.path(), "./f").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("f")).unwrap(),
            "before"
        );
    }
}
