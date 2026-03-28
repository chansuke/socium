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
