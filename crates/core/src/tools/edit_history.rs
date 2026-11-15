use crate::agent::state::{atomic_write_with_mode, load_json_file, save_json_file};
use crate::tools::workspace::{resolve_workspace_mutation_path, resolve_workspace_path_for_create};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Mutation {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// Permission bits of the file before and after the change, so a moved
    /// file and a file restored by undo get their mode back. Filled in from
    /// the file for `before` and for in-place edits; a planner sets
    /// `after_mode` for a file it creates from another, such as a move.
    /// `None` in history written before modes were kept: such records undo
    /// as they always did, without restoring a mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_mode: Option<u32>,
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
/// Undo reaches back this many edits, and no further than this many bytes
/// of saved file text, so the history never grows into the state limit.
/// Unfinished records are kept whatever their age: recovery needs them.
const MAX_EDIT_RECORDS: usize = 200;
const MAX_EDIT_HISTORY_BYTES: usize = 16 * 1024 * 1024;

impl EditRecord {
    fn bytes(&self) -> usize {
        self.changes
            .iter()
            .map(|change| {
                change.path.len()
                    + change.before.as_ref().map_or(0, String::len)
                    + change.after.as_ref().map_or(0, String::len)
            })
            .sum()
    }
}

/// Drops the oldest finished records beyond the limits.
fn prune(records: &mut Vec<EditRecord>) {
    let mut bytes: usize = records.iter().map(EditRecord::bytes).sum();
    let mut count = records.len();
    records.retain(|record| {
        let over = count > MAX_EDIT_RECORDS || bytes > MAX_EDIT_HISTORY_BYTES;
        if over && record.complete {
            count -= 1;
            bytes -= record.bytes();
            false
        } else {
            true
        }
    });
}

fn history_path(root: &Path) -> Result<PathBuf, String> {
    crate::agent::state::workspace_state_path(root, "edit_history.json").map_err(|e| e.to_string())
}
/// The file's content and permission bits, or `None` when it is missing.
fn current(path: &Path) -> Result<Option<(String, Option<u32>)>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => crate::tools::text_file::read_utf8_file_and_mode(
            path,
            &path.to_string_lossy(),
            crate::constants::MAX_READ_FILE_BYTES,
        )
        .map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Cannot inspect '{}': {e}", path.display())),
    }
}

/// Resolves the change's path and confirms the file still holds
/// `expected`, with mode `expected_mode` when one is recorded. Returns the
/// path and the mode found.
fn check(
    root: &Path,
    change: &Mutation,
    expected: &Option<String>,
    expected_mode: Option<u32>,
) -> Result<(PathBuf, Option<u32>), String> {
    let path = resolve_workspace_mutation_path(root, &change.path).map_err(|e| e.to_string())?;
    let found = current(&path)?;
    let (content, mode) = match found {
        Some((content, mode)) => (Some(content), mode),
        None => (None, None),
    };
    if content != *expected {
        return Err(format!(
            "File changed externally: '{}'; operation refused",
            change.path
        ));
    }
    if content.is_some() && expected_mode.is_some() && mode != expected_mode {
        return Err(format!(
            "File permissions changed externally: '{}'; operation refused",
            change.path
        ));
    }
    Ok((path, mode))
}

fn write(path: &Path, content: &Option<String>, mode: Option<u32>) -> Result<(), String> {
    match content {
        Some(text) => atomic_write_with_mode(path, text.as_bytes(), mode),
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
        let (resolved, mode) = check(root, change, &change.before, change.before_mode)?;
        if change.before.is_some() {
            change.before_mode = mode;
            if change.after.is_some() && change.after_mode.is_none() {
                change.after_mode = mode;
            }
        }
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
    prune(&mut history.records);
    save_json_file(&path, &history).map_err(|e| e.to_string())?;
    let mut applied = Vec::new();
    for change in &changes {
        let result = check(root, change, &change.before, change.before_mode)
            .and_then(|(path, _)| write(&path, &change.after, change.after_mode));
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
    undo_where(root, |record| {
        record
            .changes
            .iter()
            .any(|c| resolve_workspace_path_for_create(root, &c.path).is_ok_and(|p| p == target))
    })
    .map_err(|err| {
        err.unwrap_or_else(|| {
            format!(
                "No edit history found for '{requested}' (legacy history cannot be safely undone)"
            )
        })
    })
    .map(drop)
}

/// Undoes the most recent edit, whichever files it changed, and returns
/// their paths.
pub(crate) fn undo_last(root: &Path) -> Result<Vec<String>, String> {
    undo_where(root, |_| true).map_err(|err| err.unwrap_or_else(|| "No edit to undo".to_string()))
}

/// Undoes the newest record `matches` accepts. `Err(None)` means none did.
fn undo_where(
    root: &Path,
    matches: impl Fn(&EditRecord) -> bool,
) -> Result<Vec<String>, Option<String>> {
    let path = history_path(root)?;
    let mut history: EditHistoryState = load_json_file(&path).map_err(|e| e.to_string())?;
    if history.schema_version > 1 {
        return Err(Some(
            "Unsupported edit history schema; original preserved".into(),
        ));
    }
    history.schema_version = 1;
    let index = history.records.iter().rposition(matches).ok_or(None)?;
    let record = &history.records[index];
    if !record.complete {
        return Err(Some(
            "Incomplete edit or undo; inspect workspace and history before recovery".into(),
        ));
    }
    for change in &record.changes {
        check(root, change, &change.after, change.after_mode)?;
    }
    let changes = record.changes.clone();
    history.records[index].complete = false;
    save_json_file(&path, &history).map_err(|e| e.to_string())?;
    let mut restored = Vec::new();
    for change in changes.iter().rev() {
        let result = check(root, change, &change.after, change.after_mode)
            .and_then(|(path, _)| write(&path, &change.before, change.before_mode));
        if let Err(e) = result {
            return Err(Some(format!(
                "{e}; restored: {restored:?}. Incomplete undo record retained"
            )));
        }
        restored.push(change.path.clone());
    }
    history.records.remove(index);
    save_json_file(&path, &history)
        .map_err(|e| format!("Undo applied; could not save history: {e}"))?;
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(bytes: usize, complete: bool) -> EditRecord {
        EditRecord {
            changes: vec![Mutation {
                path: "f".into(),
                after: Some("x".repeat(bytes)),
                ..Mutation::default()
            }],
            complete,
        }
    }

    #[test]
    fn old_finished_records_are_dropped_and_unfinished_ones_kept() {
        // Review R2: the history grew until every edit failed at 64 MiB.
        let mut records = vec![record(10, false)];
        records.extend((0..300).map(|_| record(10, true)));
        prune(&mut records);
        assert_eq!(records.len(), MAX_EDIT_RECORDS);
        assert!(!records[0].complete, "recovery still needs it");

        let mut records = (0..100)
            .map(|_| record(1024 * 1024, true))
            .collect::<Vec<_>>();
        prune(&mut records);
        let bytes: usize = records.iter().map(EditRecord::bytes).sum();
        assert!(bytes <= MAX_EDIT_HISTORY_BYTES, "{bytes}");
        assert_eq!(records.len(), 15);
    }

    #[test]
    fn undo_last_reverts_the_newest_edit_whatever_its_files() {
        let workspace = crate::test_support::make_temp_dir("undo_last");
        let root = workspace.path();
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        for patch in [
            "*** Begin Patch\n*** Update File: a.txt\n@@\n-one\n+two\n*** End Patch",
            "*** Begin Patch\n*** Add File: b.txt\n+new\n*** Update File: a.txt\n@@\n-two\n+three\n*** End Patch",
        ] {
            crate::tools::apply_patch::apply_patch_from_workspace(
                root,
                &serde_json::json!({ "patch": patch }),
            )
            .unwrap();
        }
        let mut restored = undo_last(root).unwrap();
        restored.sort();
        assert_eq!(restored, ["a.txt", "b.txt"]);
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "two\n"
        );
        assert!(!root.join("b.txt").exists());
        undo_last(root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\n"
        );
        assert_eq!(undo_last(root).unwrap_err(), "No edit to undo");
    }

    #[cfg(unix)]
    mod modes {
        use super::super::*;
        use crate::tools::apply_patch::apply_patch_from_workspace;
        use serde_json::json;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        fn mode(path: &Path) -> u32 {
            fs::metadata(path).unwrap().permissions().mode() & 0o7777
        }

        fn chmod(path: &Path, mode: u32) {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        }

        fn patch(root: &Path, patch: &str) {
            apply_patch_from_workspace(root, &json!({ "patch": patch })).unwrap();
        }

        fn executable(root: &Path) {
            fs::write(root.join("run.sh"), "#!/bin/sh\necho old\n").unwrap();
            chmod(&root.join("run.sh"), 0o750);
        }

        const MOVE: &str = "*** Begin Patch\n*** Update File: run.sh\n*** Move to: bin/run.sh\n@@\n-echo old\n+echo new\n*** End Patch";

        #[test]
        fn move_and_its_undo_keep_the_executable_mode() {
            let root = tempfile::tempdir().unwrap();
            executable(root.path());

            patch(root.path(), MOVE);
            assert_eq!(mode(&root.path().join("bin/run.sh")), 0o750);
            assert!(!root.path().join("run.sh").exists());

            undo(root.path(), "bin/run.sh").unwrap();
            assert_eq!(mode(&root.path().join("run.sh")), 0o750);
            assert_eq!(
                fs::read_to_string(root.path().join("run.sh")).unwrap(),
                "#!/bin/sh\necho old\n"
            );
            assert!(!root.path().join("bin/run.sh").exists());
        }

        #[test]
        fn undoing_a_delete_restores_the_mode() {
            let root = tempfile::tempdir().unwrap();
            executable(root.path());

            patch(
                root.path(),
                "*** Begin Patch\n*** Delete File: run.sh\n*** End Patch",
            );
            undo(root.path(), "run.sh").unwrap();

            assert_eq!(mode(&root.path().join("run.sh")), 0o750);
        }

        #[test]
        fn an_external_chmod_blocks_undo_without_consuming_history() {
            let root = tempfile::tempdir().unwrap();
            executable(root.path());
            patch(root.path(), MOVE);
            let moved = root.path().join("bin/run.sh");
            chmod(&moved, 0o640);

            let err = undo(root.path(), "bin/run.sh").unwrap_err();
            assert!(err.contains("permissions changed externally"), "{err}");
            assert!(moved.exists() && !root.path().join("run.sh").exists());
            assert_eq!(mode(&moved), 0o640);

            // Put the mode back and the same record undoes.
            chmod(&moved, 0o750);
            undo(root.path(), "bin/run.sh").unwrap();
            assert_eq!(mode(&root.path().join("run.sh")), 0o750);
        }

        #[test]
        fn history_without_modes_still_undoes_as_before() {
            let root = tempfile::tempdir().unwrap();
            executable(root.path());
            patch(
                root.path(),
                "*** Begin Patch\n*** Update File: run.sh\n@@\n-echo old\n+echo new\n*** End Patch",
            );
            let path = history_path(root.path()).unwrap();
            let mut saved: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            for change in saved["records"][0]["changes"].as_array_mut().unwrap() {
                let change = change.as_object_mut().unwrap();
                assert!(change.remove("before_mode").is_some());
                assert!(change.remove("after_mode").is_some());
            }
            fs::write(&path, saved.to_string()).unwrap();

            undo(root.path(), "run.sh").unwrap();

            assert_eq!(
                fs::read_to_string(root.path().join("run.sh")).unwrap(),
                "#!/bin/sh\necho old\n"
            );
            assert_eq!(mode(&root.path().join("run.sh")), 0o750);
        }

        #[test]
        fn a_failed_restore_keeps_the_record_and_reports_it() {
            let root = tempfile::tempdir().unwrap();
            fs::create_dir(root.path().join("bin")).unwrap();
            fs::write(root.path().join("bin/tool"), "x\n").unwrap();
            chmod(&root.path().join("bin/tool"), 0o755);
            patch(
                root.path(),
                "*** Begin Patch\n*** Delete File: bin/tool\n*** End Patch",
            );
            chmod(&root.path().join("bin"), 0o500);

            let err = undo(root.path(), "bin/tool");
            chmod(&root.path().join("bin"), 0o755);
            let err = err.unwrap_err();
            assert!(err.contains("Incomplete undo record retained"), "{err}");
            assert!(!root.path().join("bin/tool").exists());
            // The record is not lost or replayed blindly.
            let again = undo(root.path(), "bin/tool").unwrap_err();
            assert!(again.contains("Incomplete edit or undo"), "{again}");
        }
    }
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
                ..Mutation::default()
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
