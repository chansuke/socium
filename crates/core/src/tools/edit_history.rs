use crate::agent::state::{load_json_file, save_json_file};
use crate::constants::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Default)]
struct EditHistoryState {
    entries: Vec<EditHistoryEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct EditHistoryEntry {
    pub(crate) path: String,
    pub(crate) previous_content: Option<String>,
}

fn edit_history_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(STATE_DIR_NAME)
        .join(EDIT_HISTORY_FILE_NAME)
}

pub(crate) fn record_edit_history(
    workspace_root: &Path,
    path: &str,
    previous_content: Option<String>,
) -> Result<(), String> {
    let history_path = edit_history_path(workspace_root);
    let mut history = load_json_file::<EditHistoryState>(&history_path).map_err(|e| {
        format!(
            "Failed to load edit history '{}': {e}",
            history_path.display()
        )
    })?;
    history.entries.push(EditHistoryEntry {
        path: path.to_string(),
        previous_content,
    });
    if history.entries.len() > MAX_EDIT_HISTORY_ENTRIES {
        let overflow = history.entries.len() - MAX_EDIT_HISTORY_ENTRIES;
        history.entries.drain(0..overflow);
    }
    save_json_file(&history_path, &history).map_err(|e| {
        format!(
            "Failed to save edit history '{}': {e}",
            history_path.display()
        )
    })
}

pub(crate) fn pop_edit_history(
    workspace_root: &Path,
    path: &str,
) -> Result<Option<EditHistoryEntry>, String> {
    let history_path = edit_history_path(workspace_root);
    let mut history = load_json_file::<EditHistoryState>(&history_path).map_err(|e| {
        format!(
            "Failed to load edit history '{}': {e}",
            history_path.display()
        )
    })?;
    let Some(index) = history.entries.iter().rposition(|entry| entry.path == path) else {
        return Ok(None);
    };
    let entry = history.entries.remove(index);
    save_json_file(&history_path, &history).map_err(|e| {
        format!(
            "Failed to save edit history '{}': {e}",
            history_path.display()
        )
    })?;
    Ok(Some(entry))
}
