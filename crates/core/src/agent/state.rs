use crate::agent::types::Message;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct ConversationState {
    pub messages: Vec<Message>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq)]
pub(crate) struct ConversationSummaryState {
    #[serde(default)]
    pub summary: Option<ConversationSummary>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub(crate) struct ConversationSummary {
    pub covered_messages: usize,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConversationStateRef<'a> {
    pub messages: &'a [Message],
}

pub(crate) fn load_json_file<T>(path: &Path) -> io::Result<T>
where
    T: for<'de> Deserialize<'de> + Default,
{
    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(|err| {
            Error::new(
                ErrorKind::InvalidData,
                format!("Invalid JSON in '{}': {err}", path.display()),
            )
        }),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(T::default()),
        Err(err) => Err(err),
    }
}

pub(crate) fn save_json_file<T>(path: &Path, value: &T) -> io::Result<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_string_pretty(value).map_err(|err| {
        Error::other(format!(
            "Failed to serialize JSON '{}': {err}",
            path.display()
        ))
    })?;
    fs::write(path, serialized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use serde::{Deserialize, Serialize};
    use std::fs;

    #[derive(Debug, Serialize, Deserialize, Default)]
    struct InputHistoryState {
        entries: Vec<String>,
    }

    #[test]
    fn load_json_file_returns_default_for_missing_file() {
        let temp = make_temp_dir("json-default");
        let missing = temp.path().join("missing.json");

        let state = load_json_file::<InputHistoryState>(&missing).unwrap();

        assert!(state.entries.is_empty());
    }

    #[test]
    fn load_json_file_reports_invalid_data() {
        let temp = make_temp_dir("json-invalid");
        let path = temp.path().join("invalid.json");
        fs::write(&path, "{ not-valid-json }").unwrap();

        let err = load_json_file::<InputHistoryState>(&path).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("Invalid JSON"));
        assert!(err.to_string().contains("invalid.json"));
    }

    #[test]
    fn save_json_file_creates_parent_directory_and_round_trips() {
        let temp = make_temp_dir("json-save");
        let path = temp.path().join("nested/state/history.json");
        let state = InputHistoryState {
            entries: vec!["one".to_string(), "two".to_string()],
        };

        save_json_file(&path, &state).unwrap();

        assert!(path.exists());
        let loaded = load_json_file::<InputHistoryState>(&path).unwrap();
        assert_eq!(loaded.entries, state.entries);
    }
}
