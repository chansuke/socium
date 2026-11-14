use crate::agent::types::Message;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Error, ErrorKind, Read, Write};
use std::path::{Component, Path, PathBuf};

const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;

/// Reject existing state links, including links inside the workspace. Recheck
/// every operation; this is not isolation against concurrent host-user races.
pub fn workspace_state_path(root: &Path, relative: &str) -> io::Result<PathBuf> {
    if relative.is_empty()
        || !Path::new(relative)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(Error::other("Invalid relative state path"));
    }
    let path = root.canonicalize()?.join(".socium").join(relative);
    prepare_state_path(&path, true)?;
    Ok(path)
}

pub(crate) fn check_state_file(metadata: &fs::Metadata) -> io::Result<()> {
    if !metadata.is_file() || metadata.len() > MAX_STATE_BYTES {
        return Err(Error::other(
            "State must be a regular file of at most 64 MiB",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no arguments or memory preconditions.
        if metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(Error::other(
                "State must be owned by the current user and not hard-linked",
            ));
        }
    }
    Ok(())
}

fn prepare_state_path(path: &Path, create: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::other("Missing state parent"))?;
    let mut private = false;
    for directory in parent.ancestors().collect::<Vec<_>>().into_iter().rev() {
        private |= directory.file_name().is_some_and(|name| name == ".socium");
        if !private {
            continue;
        }
        if create {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(directory) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        match fs::symlink_metadata(directory) {
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(Error::other("State directories must not be symlinks"));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::{MetadataExt, PermissionsExt};
                    // SAFETY: geteuid has no arguments or memory preconditions.
                    if metadata.uid() != unsafe { libc::geteuid() } {
                        return Err(Error::other("State directory belongs to another user"));
                    }
                    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
                }
            }
            Err(e) if !create && e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => check_state_file(&metadata)?,
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    Ok(())
}

/// Opens a private state file for appending, creating it with owner-only
/// permissions. The same symlink, ownership, and link-count checks as the
/// snapshot files apply.
pub(crate) fn open_state_append(path: &Path) -> io::Result<fs::File> {
    prepare_state_path(path, true)?;
    let mut options = fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    check_state_file(&file.metadata()?)?;
    Ok(file)
}

/// Reads a private state file of at most `limit` bytes. `None` when it does
/// not exist.
pub(crate) fn read_state_bytes(path: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    prepare_state_path(path, false)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    check_state_file(&file.metadata()?)?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::other("State file exceeds its size limit"));
    }
    Ok(Some(bytes))
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct ConversationState {
    #[serde(default)]
    pub schema_version: u32,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub events: Vec<serde_json::Value>,
    #[serde(default)]
    pub summary_state: Option<ConversationSummaryState>,
    /// Alias chosen with `/model`; `None` for the startup default.
    #[serde(default)]
    pub active_model: Option<String>,
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
    /// Raw messages from this index on were produced after this summary
    /// was in the prompt, so their thinking is still bound to the prefix it
    /// is sent with. Thinking in kept messages before it is not resent.
    /// `None` in state saved before the field existed; loading fills it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_valid_from: Option<usize>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConversationStateRef<'a> {
    pub schema_version: u32,
    pub messages: &'a [Message],
    pub events: &'a [serde_json::Value],
    pub summary_state: &'a ConversationSummaryState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_model: Option<&'a str>,
}

pub fn load_json_file<T>(path: &Path) -> io::Result<T>
where
    T: for<'de> Deserialize<'de> + Default,
{
    prepare_state_path(path, false)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    match options.open(path) {
        Ok(file) => {
            check_state_file(&file.metadata()?)?;
            let mut content = String::new();
            file.take(MAX_STATE_BYTES + 1)
                .read_to_string(&mut content)?;
            if content.len() as u64 > MAX_STATE_BYTES {
                return Err(Error::other("State exceeds 64 MiB limit"));
            }
            serde_json::from_str(&content).map_err(|err| {
                Error::new(
                    ErrorKind::InvalidData,
                    format!("Invalid JSON in '{}': {err}", path.display()),
                )
            })
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(T::default()),
        Err(err) => Err(err),
    }
}

pub fn save_json_file<T>(path: &Path, value: &T) -> io::Result<()>
where
    T: Serialize,
{
    prepare_state_path(path, true)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_string_pretty(value).map_err(|err| {
        Error::other(format!(
            "Failed to serialize JSON '{}': {err}",
            path.display()
        ))
    })?;
    if serialized.len() > 64 * 1024 * 1024 {
        return Err(Error::other(
            "State exceeds 64 MiB limit; archive the session before continuing",
        ));
    }
    atomic_write_impl(
        path,
        serialized.as_bytes(),
        ReplacementMode::Private,
        &mut |_| Ok(()),
    )
}

/// Write a same-directory temporary file, sync it, then atomically replace the
/// destination. Failed serialization/writes never truncate the previous state.
/// The file gets `mode` before it replaces the destination, so a recreated
/// file never appears with other permissions; `None` keeps the
/// destination's permissions.
pub(crate) fn atomic_write_with_mode(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
) -> io::Result<()> {
    atomic_write_impl(path, bytes, ReplacementMode::Mode(mode), &mut |_| Ok(()))
}

#[cfg(test)]
fn atomic_write_with_checkpoint(
    path: &Path,
    bytes: &[u8],
    checkpoint: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    atomic_write_impl(path, bytes, ReplacementMode::Mode(None), checkpoint)
}

/// The permissions a replacement file gets.
enum ReplacementMode {
    /// Owner-only, for private state.
    Private,
    /// These bits, or the destination's current ones when `None`.
    Mode(Option<u32>),
}

fn atomic_write_impl(
    path: &Path,
    bytes: &[u8],
    permissions: ReplacementMode,
    checkpoint: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| Error::other("missing parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".socium-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut created = false;
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        created = true;
        checkpoint("created")?;
        match permissions {
            ReplacementMode::Private => {}
            #[cfg(unix)]
            ReplacementMode::Mode(Some(mode)) => {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(mode))?;
            }
            _ => {
                if let Ok(metadata) = fs::metadata(path) {
                    file.set_permissions(metadata.permissions())?;
                }
            }
        }
        file.write_all(bytes)?;
        checkpoint("written")?;
        file.sync_all()?;
        checkpoint("synced")?;
        fs::rename(&temporary, path)?;
        checkpoint("renamed")?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if created {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn lock_workspace(root: &Path) -> io::Result<fs::File> {
    let path = workspace_state_path(root, "session.lock")?;
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    check_state_file(&file.metadata()?)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            return Err(Error::other(match lock_owner(&mut file) {
                Some(pid) => format!(
                    "Workspace session is already in use by another socium (PID {pid}); exit it or run socium in another directory"
                ),
                None => "Workspace session is already in use by another socium".to_string(),
            }));
        }
        Err(fs::TryLockError::Error(e)) => {
            return Err(Error::other(format!(
                "Workspace session cannot be locked: {e}"
            )));
        }
    }
    // Record the owner so that a second start can name it.
    file.set_len(0)?;
    writeln!(file, "{}", std::process::id())?;
    Ok(file)
}

/// The PID the lock holder recorded, when it has written one yet.
fn lock_owner(file: &mut fs::File) -> Option<u32> {
    let mut text = String::new();
    Read::by_ref(file).take(32).read_to_string(&mut text).ok()?;
    text.trim().parse().ok()
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
    fn failures_at_atomic_save_boundaries_leave_old_or_new_snapshot() {
        for boundary in ["created", "written", "synced", "renamed"] {
            let root = make_temp_dir("atomic-fault");
            let path = root.path().join("state.json");
            fs::write(&path, b"{\"generation\":1}").unwrap();
            assert!(
                atomic_write_with_checkpoint(&path, b"{\"generation\":2}", &mut |stage| if stage
                    == boundary
                {
                    Err(Error::other("injected failure"))
                } else {
                    Ok(())
                })
                .is_err()
            );
            let state: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(
                state["generation"],
                if boundary == "renamed" { 2 } else { 1 }
            );
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        }
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
