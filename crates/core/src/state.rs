use crate::conversation::Message;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Error, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

/// `.socium/scratch`, where the agent keeps temporary files out of the
/// user's tree. File tools and commands may change it, unlike the rest of
/// `.socium`. Created private, like any state directory.
pub fn scratch_dir(root: &Path) -> io::Result<PathBuf> {
    let placeholder = workspace_state_path(root, "scratch/placeholder")?;
    Ok(placeholder
        .parent()
        .expect("a state path has a parent")
        .to_path_buf())
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
    /// Name given with `/rename`.
    #[serde(default)]
    pub name: Option<String>,
    /// Random ID sent to providers with a `session_header`; `None` in state
    /// saved before the field existed, and loading makes one.
    #[serde(default)]
    pub request_session: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq)]
pub(crate) struct ConversationSummaryState {
    #[serde(default)]
    pub summary: Option<ConversationSummary>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub(crate) struct ConversationSummary {
    pub covered_messages: usize,
    pub text: String,
    /// Raw messages from this index on were produced after this summary
    /// was in the prompt, so their thinking is still bound to the prefix it
    /// is sent with. Thinking in kept messages before it is not resent.
    /// `None` in state saved before the field existed; loading fills it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_valid_from: Option<usize>,
    /// The user's first message, kept word for word (up to
    /// `MAX_FIRST_REQUEST_CHARACTERS`) beside the summary, so its later
    /// requirements survive any number of compactions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_request: Option<String>,
}

impl ConversationSummary {
    /// The first request of `messages`, for a summary that does not have
    /// one yet.
    pub(crate) fn first_request_of(messages: &[crate::conversation::Message]) -> Option<String> {
        use crate::conversation::{MessageContent, Role};
        let text = messages
            .iter()
            .find_map(|message| match (&message.role, &message.content) {
                (Role::User, MessageContent::Text(text)) => Some(text.trim()),
                _ => None,
            })?;
        Some(crate::conversation::elide_tool_output(
            text,
            crate::constants::MAX_FIRST_REQUEST_CHARACTERS,
            "from the first request",
        ))
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ConversationStateRef<'a> {
    pub schema_version: u32,
    pub messages: &'a [Message],
    pub events: &'a [serde_json::Value],
    pub summary_state: &'a ConversationSummaryState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_session: Option<&'a str>,
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
        return Err(Error::other(format!(
            "'{}' would exceed the 64 MiB state limit; /clear starts a new conversation",
            path.display()
        )));
    }
    atomic_write_impl(
        path,
        serialized.as_bytes(),
        ReplacementMode::Private,
        &mut |_| Ok(()),
    )
}

/// Atomically replaces a private state file with `bytes`.
pub(crate) fn save_state_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    prepare_state_path(path, true)?;
    atomic_write_impl(path, bytes, ReplacementMode::Private, &mut |_| Ok(()))
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

/// How long a start waits for a process this one started to drop its copy
/// of a released lock, and how often it looks.
const LOCK_RELEASE_WAIT: Duration = Duration::from_secs(2);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// The lock's holder is a child this process started that has not run its
/// program yet, and nothing reports when it lets go of the lock.
#[expect(
    clippy::disallowed_methods,
    reason = "no event reports when a child releases its copy of the lock"
)]
fn wait_before_retrying_lock() {
    std::thread::sleep(LOCK_RETRY_INTERVAL);
}

/// Lock files this process holds. A process another thread starts keeps a
/// copy of every open file descriptor from fork until exec, and the lock
/// with it, so a released lock can still look held for that moment.
static HELD_LOCKS: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

fn held_locks() -> std::sync::MutexGuard<'static, BTreeSet<PathBuf>> {
    HELD_LOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The workspace lock, held until dropped.
#[derive(Debug)]
pub(crate) struct WorkspaceLock {
    file: fs::File,
    path: PathBuf,
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        held_locks().remove(&self.path);
    }
}

pub(crate) fn lock_workspace(root: &Path) -> io::Result<WorkspaceLock> {
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
    let mut file = options.open(&path)?;
    check_state_file(&file.metadata()?)?;
    let in_use = |pid: Option<u32>| {
        Error::other(match pid {
            Some(pid) => format!(
                "Workspace session is already in use by another socium (PID {pid}); exit it or run socium in another directory"
            ),
            None => "Workspace session is already in use by another socium".to_string(),
        })
    };
    let deadline = Instant::now() + LOCK_RELEASE_WAIT;
    loop {
        if held_locks().contains(&path) {
            return Err(in_use(Some(std::process::id())));
        }
        match file.try_lock() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) => {
                let owner = lock_owner(&mut file);
                // This process wrote the PID and no longer holds the lock:
                // a child it started has a copy until it runs.
                if owner != Some(std::process::id()) || Instant::now() >= deadline {
                    return Err(in_use(owner));
                }
                wait_before_retrying_lock();
            }
            Err(fs::TryLockError::Error(e)) => {
                return Err(Error::other(format!(
                    "Workspace session cannot be locked: {e}"
                )));
            }
        }
    }
    held_locks().insert(path.clone());
    let lock = WorkspaceLock { file, path };
    // Record the owner so that a second start can name it.
    let mut file = &lock.file;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    writeln!(file, "{}", std::process::id())?;
    Ok(lock)
}

/// The PID the lock holder recorded, when it has written one yet.
fn lock_owner(file: &mut fs::File) -> Option<u32> {
    file.seek(SeekFrom::Start(0)).ok()?;
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

    #[cfg(unix)]
    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the child holds its copy of the lock for a while on purpose"
    )]
    fn the_lock_can_be_taken_again_while_a_child_started_under_it_has_not_run_yet() {
        use std::os::unix::process::CommandExt;
        let root = make_temp_dir("lock-child");
        let lock = lock_workspace(root.path()).unwrap();
        // A process another thread starts gets a copy of the lock's file
        // descriptor, which it keeps from fork until exec.
        let child = std::thread::spawn(|| {
            let mut command = std::process::Command::new("sh");
            command.args(["-c", "exit 0"]);
            // SAFETY: the hook only sleeps, which is async-signal-safe.
            unsafe {
                command.pre_exec(|| {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    Ok(())
                });
            }
            command.status().unwrap()
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(lock);

        let again = lock_workspace(root.path());

        assert!(child.join().unwrap().success());
        assert!(again.is_ok(), "{:?}", again.err());
        // The lock still keeps out a second holder in this process.
        assert!(lock_workspace(root.path()).is_err());
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

    mod properties {
        use super::super::*;
        use crate::conversation::{ContentBlock, Message, MessageContent, Origin, Role};
        use crate::test_support::make_temp_dir;
        use proptest::prelude::*;
        use serde_json::Value;

        fn text() -> impl Strategy<Value = String> {
            "\\PC{0,12}"
        }

        /// Tool input as a model sends it. Floats are left out: serde_json
        /// may parse one back a unit in the last place off, which no tool
        /// depends on.
        fn tool_input() -> impl Strategy<Value = Value> {
            let leaf = prop_oneof![
                Just(Value::Null),
                any::<bool>().prop_map(Value::from),
                any::<i64>().prop_map(Value::from),
                any::<u64>().prop_map(Value::from),
                text().prop_map(Value::from),
            ];
            leaf.prop_recursive(3, 16, 4, |inner| {
                prop_oneof![
                    prop::collection::vec(inner.clone(), 0..4).prop_map(Value::from),
                    prop::collection::btree_map(text(), inner, 0..4)
                        .prop_map(|map| Value::Object(map.into_iter().collect())),
                ]
            })
        }

        fn block() -> impl Strategy<Value = ContentBlock> {
            prop_oneof![
                text().prop_map(|text| ContentBlock::Text { text }),
                (text(), text()).prop_map(|(thinking, signature)| ContentBlock::Thinking {
                    thinking,
                    signature
                }),
                text().prop_map(|data| ContentBlock::RedactedThinking { data }),
                (text(), text(), tool_input())
                    .prop_map(|(id, name, input)| ContentBlock::ToolUse { id, name, input }),
                (text(), text(), prop::option::of(any::<bool>())).prop_map(
                    |(tool_use_id, content, is_error)| {
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        }
                    }
                ),
            ]
        }

        fn message() -> impl Strategy<Value = Message> {
            let role = prop_oneof![Just(Role::User), Just(Role::Assistant)];
            let content = prop_oneof![
                text().prop_map(MessageContent::Text),
                prop::collection::vec(block(), 0..4).prop_map(MessageContent::Blocks),
            ];
            let origin = prop::option::of(
                (text(), text()).prop_map(|(provider, model)| Origin { provider, model }),
            );
            (role, content, origin).prop_map(|(role, content, origin)| Message {
                role,
                content,
                origin,
            })
        }

        fn summary() -> impl Strategy<Value = ConversationSummaryState> {
            prop::option::of(
                (
                    any::<usize>(),
                    text(),
                    prop::option::of(any::<usize>()),
                    prop::option::of(text()),
                )
                    .prop_map(
                        |(covered_messages, text, thinking_valid_from, first_request)| {
                            ConversationSummary {
                                covered_messages,
                                text,
                                thinking_valid_from,
                                first_request,
                            }
                        },
                    ),
            )
            .prop_map(|summary| ConversationSummaryState { summary })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(64))]

            /// What a save writes, a resume reads back unchanged: the saving and loading types must not drift apart.
            #[test]
            fn a_saved_conversation_loads_back_unchanged(
                messages in prop::collection::vec(message(), 0..5),
                events in prop::collection::vec(tool_input(), 0..3),
                summary_state in summary(),
                active_model in prop::option::of(text()),
                name in prop::option::of(text()),
            ) {
                let temp = make_temp_dir("round-trip");
                let path = temp.path().join("conversation.json");
                let saved = ConversationStateRef {
                    schema_version: 1,
                    messages: &messages,
                    events: &events,
                    summary_state: &summary_state,
                    active_model: active_model.as_deref(),
                    name: name.as_deref(),
                    request_session: None,
                };
                save_json_file(&path, &saved).unwrap();
                let loaded = load_json_file::<ConversationState>(&path).unwrap();
                prop_assert_eq!(loaded.schema_version, 1);
                prop_assert_eq!(&loaded.messages, &messages);
                prop_assert_eq!(&loaded.events, &events);
                prop_assert_eq!(loaded.summary_state.as_ref(), Some(&summary_state));
                prop_assert_eq!(&loaded.active_model, &active_model);
                prop_assert_eq!(&loaded.name, &name);
            }
        }
    }
}
