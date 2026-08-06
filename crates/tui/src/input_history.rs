use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::{Path, PathBuf};

const MAX_INPUT_HISTORY_ENTRIES: usize = 1_000;

fn load_json_file<T>(path: &Path) -> io::Result<T>
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

fn save_json_file<T>(path: &Path, value: &T) -> io::Result<()>
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

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct InputHistoryState {
    pub entries: Vec<String>,
}

#[derive(Debug, Serialize)]
struct InputHistoryStateRef<'a> {
    entries: &'a [String],
}

pub struct InputSession {
    pub history_path: PathBuf,
    pub entries: Vec<String>,
}

impl InputSession {
    pub fn load(history_path: PathBuf) -> io::Result<Self> {
        let state = load_json_file::<InputHistoryState>(&history_path)?;
        Ok(Self {
            history_path,
            entries: state.entries,
        })
    }

    pub fn record_entry(&mut self, entry: &str) -> io::Result<()> {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        if self.entries.last().is_some_and(|last| last == entry) {
            return Ok(());
        }

        self.entries.push(entry.to_string());
        if self.entries.len() > MAX_INPUT_HISTORY_ENTRIES {
            let overflow = self.entries.len() - MAX_INPUT_HISTORY_ENTRIES;
            self.entries.drain(0..overflow);
        }

        save_json_file(
            &self.history_path,
            &InputHistoryStateRef {
                entries: &self.entries,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn input_session_record_entry_ignores_empty_and_whitespace() {
        let temp = make_temp_dir("input-history-empty");
        let history_path = temp.path().join("history.json");
        let mut session = InputSession {
            history_path: history_path.clone(),
            entries: Vec::new(),
        };

        session.record_entry("").unwrap();
        session.record_entry("   ").unwrap();

        assert!(session.entries.is_empty());
        assert!(!history_path.exists());
    }

    #[test]
    fn input_session_record_entry_skips_duplicate_consecutive() {
        let temp = make_temp_dir("input-history-dup");
        let history_path = temp.path().join("history.json");
        let mut session = InputSession {
            history_path: history_path.clone(),
            entries: vec!["same".to_string()],
        };

        session.record_entry("same").unwrap();

        assert_eq!(session.entries, vec!["same".to_string()]);
        assert!(!history_path.exists());
    }

    #[test]
    fn input_session_record_entry_trims_to_max_entries() {
        let temp = make_temp_dir("input-history-limit");
        let history_path = temp.path().join("history.json");
        let mut entries = Vec::with_capacity(MAX_INPUT_HISTORY_ENTRIES);
        for i in 0..MAX_INPUT_HISTORY_ENTRIES {
            entries.push(format!("entry-{i}"));
        }
        let mut session = InputSession {
            history_path: history_path.clone(),
            entries,
        };

        session.record_entry("entry-extra").unwrap();

        assert_eq!(session.entries.len(), MAX_INPUT_HISTORY_ENTRIES);
        assert_eq!(session.entries.first().unwrap(), "entry-1");
        assert_eq!(session.entries.last().unwrap(), "entry-extra");

        let state = load_json_file::<InputHistoryState>(&history_path).unwrap();
        assert_eq!(state.entries, session.entries);
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn make_temp_dir(prefix: &str) -> TempDir {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("socium_{prefix}_{nanos}_{count}"));
        fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }
}
