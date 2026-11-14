//! Conversations kept side by side in one workspace, each under
//! `.socium/sessions/<id>/`, and the move of the single pre-session
//! conversation into one of them.

use crate::activity_log::{ACTIVITY_LOG_FILE_NAME, ROTATED_FILE_NAME};
use crate::agent::state::{check_state_file, load_json_file, lock_workspace, workspace_state_path};
use crate::agent::types::{ContentBlock, Message, MessageContent, Role};
use crate::constants::{CONVERSATION_STATE_FILE_NAME, STATE_DIR_NAME};
use serde::Deserialize;
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const SESSIONS_DIR_NAME: &str = "sessions";
const MAX_PREVIEW_CHARACTERS: usize = 200;
const MAX_NAME_CHARACTERS: usize = 80;

/// Which conversation a start opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStart {
    /// A fresh conversation, written once it has something to save.
    New,
    /// The most recently saved conversation, or a fresh one when none has
    /// any messages.
    Continue,
    /// The session with this ID or unique ID prefix.
    Resume(String),
}

/// A saved session as the resume picker lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: String,
    /// When its conversation was last saved.
    pub updated: SystemTime,
    pub message_count: usize,
    /// First line of the first user prompt, or why the session cannot be
    /// read.
    pub preview: String,
    /// The model alias chosen with `/model` in it.
    pub active_model: Option<String>,
    /// Name given with `/rename`.
    pub name: Option<String>,
    pub readable: bool,
}

/// The part of a conversation snapshot the listing needs.
#[derive(Deserialize, Default)]
struct SessionHead {
    #[serde(default)]
    messages: Vec<Message>,
    #[serde(default)]
    active_model: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// `YYYYMMDD-HHMMSS-xxxx` in UTC; the suffix is taken from the sub-second
/// part so IDs made in the same second differ.
pub(crate) fn session_id_at(time: SystemTime) -> String {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let of_day = seconds % 86_400;
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{:04x}",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60,
        (since_epoch.subsec_nanos() >> 14) & 0xffff
    )
}

/// Proleptic Gregorian date of a day count since 1970-01-01, from Howard
/// Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

pub(crate) fn is_session_id(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 20
        && bytes.iter().enumerate().all(|(i, b)| match i {
            8 | 15 => *b == b'-',
            0..8 | 9..15 => b.is_ascii_digit(),
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(b),
        })
}

fn sessions_dir(root: &Path) -> PathBuf {
    root.join(STATE_DIR_NAME).join(SESSIONS_DIR_NAME)
}

/// Where a session keeps `file`. Nothing is created.
pub(crate) fn session_file(root: &Path, id: &str, file: &str) -> PathBuf {
    sessions_dir(root).join(id).join(file)
}

/// A fresh ID whose directory does not exist yet and that is not `avoid`.
pub(crate) fn new_session_id(root: &Path, avoid: Option<&str>) -> String {
    let mut time = SystemTime::now();
    loop {
        let id = session_id_at(time);
        if Some(id.as_str()) != avoid && fs::symlink_metadata(sessions_dir(root).join(&id)).is_err()
        {
            return id;
        }
        time += std::time::Duration::from_micros(16);
    }
}

/// Moves the pre-session `.socium/conversation.json` and its Activity log
/// into a session named after the conversation's modification time, and
/// returns that ID. Files are renamed, never copied or overwritten; the
/// conversation moves last, so a move cut short resumes on the next start.
/// Call with the workspace lock held.
pub(crate) fn migrate_legacy_conversation(root: &Path) -> io::Result<Option<String>> {
    let state_dir = root.join(STATE_DIR_NAME);
    let legacy = state_dir.join(CONVERSATION_STATE_FILE_NAME);
    let metadata = match fs::symlink_metadata(&legacy) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    check_state_file(&metadata)?;
    let id = session_id_at(metadata.modified()?);
    // Creates the private session directory and checks the destination.
    workspace_state_path(
        root,
        &format!("{SESSIONS_DIR_NAME}/{id}/{CONVERSATION_STATE_FILE_NAME}"),
    )?;
    for file in [
        ROTATED_FILE_NAME,
        ACTIVITY_LOG_FILE_NAME,
        CONVERSATION_STATE_FILE_NAME,
    ] {
        let from = state_dir.join(file);
        match fs::symlink_metadata(&from) {
            Ok(metadata) => check_state_file(&metadata)?,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        }
        let to = session_file(root, &id, file);
        if fs::symlink_metadata(&to).is_ok() {
            return Err(Error::other(format!(
                "Cannot move '{}' into session {id}: '{}' already exists; move one of them aside",
                from.display(),
                to.display()
            )));
        }
        fs::rename(&from, &to)?;
    }
    #[cfg(unix)]
    {
        fs::File::open(sessions_dir(root).join(&id))?.sync_all()?;
        fs::File::open(&state_dir)?.sync_all()?;
    }
    Ok(Some(id))
}

/// Saved sessions, most recently saved first. Sessions without a saved
/// conversation are left out; an unreadable one is listed with the reason
/// in its preview. Call with the workspace lock held.
pub(crate) fn scan_sessions(root: &Path) -> io::Result<Vec<SessionSummary>> {
    let entries = match fs::read_dir(sessions_dir(root)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut sessions = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !is_session_id(&id) || !entry.file_type()?.is_dir() {
            continue;
        }
        let path = session_file(root, &id, CONVERSATION_STATE_FILE_NAME);
        let updated = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata.modified()?,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        sessions.push(match load_json_file::<SessionHead>(&path) {
            Ok(head) => SessionSummary {
                id,
                updated,
                message_count: head.messages.len(),
                preview: preview(&head.messages),
                active_model: head.active_model,
                name: head.name,
                readable: true,
            },
            Err(err) => SessionSummary {
                id,
                updated,
                message_count: 0,
                preview: format!("Cannot read: {err}"),
                active_model: None,
                name: None,
                readable: false,
            },
        });
    }
    sessions.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
    Ok(sessions)
}

/// First non-blank line the user typed; tool results are not prompts.
fn preview(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|message| matches!(message.role, Role::User))
        .flat_map(|message| match &message.content {
            MessageContent::Text(text) => vec![text.as_str()],
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect(),
        })
        .flat_map(str::lines)
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .take(MAX_PREVIEW_CHARACTERS)
        .collect()
}

/// The session `query` names: its ID, else its name, else a prefix of its
/// ID.
pub(crate) fn resolve_session<'a>(
    sessions: &'a [SessionSummary],
    query: &str,
) -> io::Result<&'a SessionSummary> {
    if let Some(exact) = sessions.iter().find(|session| session.id == query) {
        return Ok(exact);
    }
    let named = sessions
        .iter()
        .filter(|session| session.name.as_deref() == Some(query))
        .collect::<Vec<_>>();
    let matches = if named.is_empty() {
        sessions
            .iter()
            .filter(|session| !query.is_empty() && session.id.starts_with(query))
            .collect::<Vec<_>>()
    } else {
        named
    };
    match matches.as_slice() {
        [only] => Ok(only),
        [] => Err(Error::new(
            ErrorKind::NotFound,
            format!("No saved session '{query}' in this workspace"),
        )),
        several => Err(Error::new(
            ErrorKind::InvalidInput,
            format!(
                "Session '{query}' is ambiguous: {}",
                several
                    .iter()
                    .map(|session| session.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// A session name: trimmed, one line, at most `MAX_NAME_CHARACTERS`.
pub(crate) fn validate_session_name(name: &str) -> io::Result<String> {
    let name = name.trim();
    let invalid = |reason: &str| Err(Error::new(ErrorKind::InvalidInput, reason.to_string()));
    if name.is_empty() {
        return invalid("A session name cannot be empty");
    }
    if name.chars().count() > MAX_NAME_CHARACTERS {
        return invalid("A session name can have at most 80 characters");
    }
    if name.chars().any(char::is_control) {
        return invalid("A session name must be one line without control characters");
    }
    Ok(name.to_string())
}

/// Removes the session directory with this exact ID. Call with the
/// workspace lock held.
pub(crate) fn delete_session_dir(root: &Path, id: &str) -> io::Result<()> {
    let not_found = || {
        Error::new(
            ErrorKind::NotFound,
            format!("No saved session '{id}' in this workspace"),
        )
    };
    if !is_session_id(id) {
        return Err(not_found());
    }
    let dir = sessions_dir(root).join(id);
    match fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&dir),
        Ok(_) => Err(Error::other(format!(
            "'{}' is not a session directory; inspect it by hand",
            dir.display()
        ))),
        Err(err) if err.kind() == ErrorKind::NotFound => Err(not_found()),
        Err(err) => Err(err),
    }
}

/// Saved sessions with messages, most recently saved first, for choosing
/// one to resume. Briefly takes the workspace lock, so it fails while
/// another Socium runs here.
pub fn list_sessions(root: &Path) -> io::Result<Vec<SessionSummary>> {
    let _lock = lock_workspace(root)?;
    migrate_legacy_conversation(root)?;
    Ok(scan_sessions(root)?
        .into_iter()
        .filter(|session| session.message_count > 0 || !session.readable)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use std::time::Duration;

    fn at(seconds: u64, nanos: u32) -> SystemTime {
        UNIX_EPOCH + Duration::new(seconds, nanos)
    }

    fn save_conversation(root: &Path, id: &str, messages: &[&str], mtime: u64) {
        let path = workspace_state_path(
            root,
            &format!("{SESSIONS_DIR_NAME}/{id}/{CONVERSATION_STATE_FILE_NAME}"),
        )
        .unwrap();
        let messages = messages
            .iter()
            .map(|text| serde_json::json!({"role": "user", "content": text}))
            .collect::<Vec<_>>();
        fs::write(
            &path,
            serde_json::json!({"schema_version": 1, "messages": messages}).to_string(),
        )
        .unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(at(mtime, 0))
            .unwrap();
    }

    #[test]
    fn ids_are_utc_timestamps_with_a_sub_second_suffix() {
        // 2026-11-14T15:30:12Z
        let id = session_id_at(at(1_794_670_212, 0));
        assert_eq!(id, "20261114-153012-0000");
        assert!(is_session_id(&id));
        assert_eq!(session_id_at(at(0, 999_999_999)), "19700101-000000-ee6b");
        assert_eq!(session_id_at(at(951_782_400, 0)), "20000229-000000-0000");
        for name in ["", "legacy", "20261114-153012-00", "20261114_153012-0000"] {
            assert!(!is_session_id(name), "{name}");
        }
        assert!(!is_session_id("20261114-153012-00G0"));
    }

    #[test]
    fn new_ids_skip_existing_directories_and_the_current_one() {
        let root = make_temp_dir("session-ids");
        let first = new_session_id(root.path(), None);
        fs::create_dir_all(sessions_dir(root.path()).join(&first)).unwrap();
        let second = new_session_id(root.path(), Some(&first));
        assert_ne!(first, second);
        assert!(is_session_id(&second));
    }

    #[test]
    fn the_legacy_conversation_and_its_activity_move_into_a_session() {
        let root = make_temp_dir("session-migrate");
        let state = workspace_state_path(root.path(), CONVERSATION_STATE_FILE_NAME).unwrap();
        fs::write(
            &state,
            r#"{"schema_version":1,"messages":[{"role":"user","content":"hi"}]}"#,
        )
        .unwrap();
        fs::File::options()
            .write(true)
            .open(&state)
            .unwrap()
            .set_modified(at(1_794_670_212, 0))
            .unwrap();
        let state_dir = root.path().join(STATE_DIR_NAME);
        fs::write(state_dir.join(ACTIVITY_LOG_FILE_NAME), "new\n").unwrap();
        fs::write(state_dir.join(ROTATED_FILE_NAME), "old\n").unwrap();

        let id = migrate_legacy_conversation(root.path()).unwrap().unwrap();

        assert_eq!(id, "20261114-153012-0000");
        assert!(!state.exists());
        assert_eq!(
            fs::read_to_string(session_file(root.path(), &id, ACTIVITY_LOG_FILE_NAME)).unwrap(),
            "new\n"
        );
        assert_eq!(
            fs::read_to_string(session_file(root.path(), &id, ROTATED_FILE_NAME)).unwrap(),
            "old\n"
        );
        let sessions = scan_sessions(root.path()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].preview, "hi");
        assert_eq!(migrate_legacy_conversation(root.path()).unwrap(), None);
    }

    #[test]
    fn a_migration_cut_short_finishes_and_a_conflict_overwrites_nothing() {
        let root = make_temp_dir("session-migrate-resume");
        let state = workspace_state_path(root.path(), CONVERSATION_STATE_FILE_NAME).unwrap();
        fs::write(&state, r#"{"messages":[]}"#).unwrap();
        let id = session_id_at(fs::metadata(&state).unwrap().modified().unwrap());
        // The Activity log already moved before a crash.
        let moved = workspace_state_path(
            root.path(),
            &format!("{SESSIONS_DIR_NAME}/{id}/{ACTIVITY_LOG_FILE_NAME}"),
        )
        .unwrap();
        fs::write(&moved, "moved\n").unwrap();

        assert_eq!(
            migrate_legacy_conversation(root.path()).unwrap(),
            Some(id.clone())
        );
        assert_eq!(fs::read_to_string(&moved).unwrap(), "moved\n");

        // A second legacy file with the same name as a moved one is refused.
        fs::write(&state, r#"{"messages":[]}"#).unwrap();
        fs::File::options()
            .write(true)
            .open(&state)
            .unwrap()
            .set_modified(
                fs::metadata(session_file(root.path(), &id, CONVERSATION_STATE_FILE_NAME))
                    .unwrap()
                    .modified()
                    .unwrap(),
            )
            .unwrap();
        let err = migrate_legacy_conversation(root.path()).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        assert!(state.exists());
    }

    #[test]
    fn listing_is_newest_first_and_skips_unsaved_and_empty_sessions() {
        let root = make_temp_dir("session-list");
        save_conversation(
            root.path(),
            "20261101-000000-0000",
            &["old\nsecond line"],
            100,
        );
        save_conversation(root.path(), "20261102-000000-0000", &["", "  new  "], 200);
        save_conversation(root.path(), "20261103-000000-0000", &[], 300);
        fs::create_dir_all(sessions_dir(root.path()).join("20261104-000000-0000")).unwrap();
        fs::create_dir_all(sessions_dir(root.path()).join("not-a-session")).unwrap();

        let all = scan_sessions(root.path()).unwrap();
        assert_eq!(
            all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            [
                "20261103-000000-0000",
                "20261102-000000-0000",
                "20261101-000000-0000"
            ]
        );

        let listed = list_sessions(root.path()).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|s| (s.id.as_str(), s.message_count, s.preview.as_str()))
                .collect::<Vec<_>>(),
            [
                ("20261102-000000-0000", 2, "new"),
                ("20261101-000000-0000", 1, "old"),
            ]
        );
    }

    #[test]
    fn an_unreadable_session_is_listed_with_the_reason() {
        let root = make_temp_dir("session-unreadable");
        save_conversation(root.path(), "20261101-000000-0000", &["ok"], 100);
        let broken = workspace_state_path(
            root.path(),
            &format!("{SESSIONS_DIR_NAME}/20261102-000000-0000/{CONVERSATION_STATE_FILE_NAME}"),
        )
        .unwrap();
        fs::write(&broken, "{").unwrap();

        let listed = list_sessions(root.path()).unwrap();
        assert_eq!(listed.len(), 2);
        let unreadable = listed.iter().find(|s| !s.readable).unwrap();
        assert!(
            unreadable.preview.starts_with("Cannot read:"),
            "{}",
            unreadable.preview
        );
    }

    #[test]
    fn a_session_is_named_by_its_id_or_a_unique_prefix() {
        let root = make_temp_dir("session-resolve");
        save_conversation(root.path(), "20261101-000000-0000", &["a"], 100);
        save_conversation(root.path(), "20261101-000000-0001", &["b"], 200);
        save_conversation(root.path(), "20261102-000000-0000", &["c"], 300);
        let sessions = scan_sessions(root.path()).unwrap();

        assert_eq!(
            resolve_session(&sessions, "20261101-000000-0000")
                .unwrap()
                .id,
            "20261101-000000-0000"
        );
        assert_eq!(
            resolve_session(&sessions, "20261102").unwrap().id,
            "20261102-000000-0000"
        );
        let ambiguous = resolve_session(&sessions, "20261101").unwrap_err();
        assert_eq!(ambiguous.kind(), ErrorKind::InvalidInput);
        assert!(ambiguous.to_string().contains("20261101-000000-0001"));
        for missing in ["", "2027", "../x"] {
            assert_eq!(
                resolve_session(&sessions, missing).unwrap_err().kind(),
                ErrorKind::NotFound,
                "{missing}"
            );
        }
    }

    #[test]
    fn listing_fails_while_another_socium_holds_the_workspace() {
        let root = make_temp_dir("session-locked");
        let _held = lock_workspace(root.path()).unwrap();
        assert!(list_sessions(root.path()).is_err());
    }
}
