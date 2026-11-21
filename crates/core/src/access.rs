//! What a session grant covers and which workspace files a tool must not
//! show or change without review, and the listing that finds them before
//! and after a command nobody reviewed. `approval` decides whether a call
//! asks; the tools read this to withhold files and to quarantine what such a
//! command left, so it sits below both.

use crate::settings::PathPatterns;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Something the user allowed for the rest of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    /// Commands whose arguments start with these.
    Command(Vec<String>),
    /// Edits to these workspace paths.
    Edit(Vec<String>),
    /// Reads of these workspace paths.
    Read(Vec<String>),
}

impl Grant {
    /// What the grant covers, as the approval picker names it after
    /// "Allow".
    pub fn scope(&self) -> String {
        match self {
            Grant::Command(prefix) => format!("`{}` commands", prefix.join(" ")),
            Grant::Edit(paths) => format!("edits to {}", paths.join(", ")),
            Grant::Read(paths) => format!("reading {}", paths.join(", ")),
        }
    }

    fn covers(&self, other: &Grant) -> bool {
        match (self, other) {
            (Grant::Command(prefix), Grant::Command(argv)) => argv.starts_with(prefix),
            (Grant::Edit(granted), Grant::Edit(paths))
            | (Grant::Read(granted), Grant::Read(paths)) => {
                paths.iter().all(|path| granted.contains(path))
            }
            _ => false,
        }
    }
}

/// Whether a search or diff of `named` leaves out the contents of `path`,
/// both relative to the workspace: a file that may hold credentials, unless
/// it is the one named, whose read was approved already.
pub(crate) fn withholds(
    sensitive: &crate::settings::PathPatterns,
    path: &str,
    named: &str,
) -> bool {
    path != named && sensitive.matches(path)
}

/// The note under a result that left out such files.
pub(crate) fn withheld_note(paths: &[String]) -> String {
    format!(
        "Left out {} that may hold credentials: {}. Name one as the path to read it, which asks for approval.",
        if paths.len() == 1 { "a file" } else { "files" },
        paths.join(", ")
    )
}

/// Whether a session grant covers `wanted`.
pub(crate) fn grants_cover(grants: &[Grant], wanted: &Grant) -> bool {
    grants.iter().any(|grant| grant.covers(wanted))
}

/// `requested` as it resolves inside the workspace, symlinks followed, so
/// an alias cannot dodge a pattern. `None` when it does not resolve; the
/// tool then reports the error.
pub(crate) fn relative_target(root: &Path, requested: &str) -> Option<String> {
    let resolved = crate::workspace::resolve_workspace_path_for_create(root, requested).ok()?;
    let root = root.canonicalize().ok()?;
    let relative = resolved.strip_prefix(&root).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

/// How long listing the workspace's protected and sensitive files may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);

/// What the sandbox withholds from a command nobody reviewed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Restrictions {
    pub(crate) hidden: Vec<PathBuf>,
    pub(crate) frozen: Vec<PathBuf>,
    /// Freezing covers only files that exist, so the protected files at the
    /// start are kept to find any the command created or replaced.
    pub(crate) protected: PathPatterns,
    pub(crate) before: BTreeMap<String, FileId>,
    /// Directory symlinks at the start, so the listing after the command
    /// follows the ones it made or changed.
    pub(crate) links: BTreeMap<String, FileId>,
}

/// A path's device, inode and change time, so a file replaced at the same
/// path shows even when the new one gets the old inode.
pub(crate) type FileId = (u64, u64, i64, i64);

fn file_id(metadata: &std::fs::Metadata) -> FileId {
    use std::os::unix::fs::MetadataExt;
    (
        metadata.dev(),
        metadata.ino(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

/// What a listing found.
#[derive(Default)]
pub(crate) struct Listed {
    /// Matching files and symlinks, by the path host tools find them at.
    pub(crate) files: Vec<ListedFile>,
    /// Every directory symlink.
    pub(crate) links: BTreeMap<String, FileId>,
    /// Followed directory symlinks with too much behind them to list.
    pub(crate) unlisted_links: Vec<String>,
}

pub(crate) struct ListedFile {
    pub(crate) path: String,
    pub(crate) id: FileId,
    /// The directory symlink the file was reached through: it, not the
    /// file behind it, is what the workspace holds.
    pub(crate) through: Option<String>,
}

impl Restrictions {
    /// Moves protected files the command created or replaced into
    /// `.socium/quarantine`, where no host tool finds them and no command
    /// reaches them, and says what it moved and what is gone. `None` when
    /// the command left them as they were.
    pub(crate) fn withhold_changes(&self, root: &Path) -> Result<Option<String>, String> {
        let after = find_matching_files(
            root,
            &|path| self.protected.matches(path),
            Listing::After(&self.links),
        )?;
        let mut changed = BTreeMap::new();
        for file in &after.files {
            if self.before.get(&file.path) != Some(&file.id) {
                let (path, id) = match &file.through {
                    Some(link) => (link, after.links[link]),
                    None => (&file.path, file.id),
                };
                changed.insert(path.clone(), Some(id));
            }
        }
        for link in &after.unlisted_links {
            changed.insert(link.clone(), None);
        }
        let missing = self
            .before
            .keys()
            .filter(|path| !after.files.iter().any(|kept| kept.path == **path))
            .cloned()
            .collect::<Vec<_>>();
        if changed.is_empty() && missing.is_empty() {
            return Ok(None);
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut moved = Vec::new();
        let mut removed = Vec::new();
        for (path, id) in &changed {
            let source = root.join(path);
            let parent = source.parent().unwrap_or(root);
            let quarantined = with_owner_access(parent, || {
                crate::state::workspace_state_path(root, &format!("quarantine/{stamp}/{path}"))
                    .and_then(|destination| std::fs::rename(&source, destination))
            });
            if quarantined.is_ok() {
                moved.push(path.as_str());
            } else {
                // Such as on a full disk or another file system. Only the
                // file that was listed is removed, not one saved since.
                let unchanged = std::fs::symlink_metadata(&source)
                    .is_ok_and(|metadata| id.is_none_or(|id| file_id(&metadata) == id));
                if !unchanged {
                    return Err(format!("{path} changed while Socium checked it"));
                }
                with_owner_access(parent, || std::fs::remove_file(&source)).map_err(|err| {
                    format!("{path} could not be moved out of the workspace or removed ({err})")
                })?;
                removed.push(path.as_str());
            }
        }
        let mut note = String::new();
        let rule = "a command that runs without review may not create or replace files that host tools run on their own. Write them with the file tools, which ask the user first.";
        if !moved.is_empty() {
            note.push_str(&format!(
                "\nSocium moved {} to .socium/quarantine/{stamp}/: {rule}",
                moved.join(", ")
            ));
        }
        if !removed.is_empty() {
            note.push_str(&format!(
                "\nSocium removed {}, which it could not quarantine: {rule}",
                removed.join(", ")
            ));
        }
        if !missing.is_empty() {
            note.push_str(&format!(
                "\n{} {} no longer where the command found {}.",
                missing.join(", "),
                if missing.len() == 1 { "is" } else { "are" },
                if missing.len() == 1 { "it" } else { "them" },
            ));
        }
        Ok(Some(note))
    }
}

/// Runs `op`, and once more after giving the owner full access to `dir`
/// if it was refused: a command can take that access away from a directory
/// it made, to keep its files out of reach.
fn with_owner_access<T>(dir: &Path, op: impl Fn() -> std::io::Result<T>) -> std::io::Result<T> {
    use std::os::unix::fs::PermissionsExt;
    match op() {
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            let mode = std::fs::symlink_metadata(dir)?.permissions().mode();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode | 0o700))?;
            op()
        }
        result => result,
    }
}

/// Most entries a listing reads behind followed directory symlinks.
const MAX_LINKED_ENTRIES: usize = 10_000;

/// Whether a listing runs before a command or after it.
#[derive(Clone, Copy)]
pub(crate) enum Listing<'a> {
    /// May give up, since the command then asks instead.
    Before,
    /// Must see all the command left, so it never gives up, gives back the
    /// owner access the command took from a directory, and follows the
    /// directory symlinks that are not among these from before.
    After(&'a BTreeMap<String, FileId>),
}

/// Workspace files and symlinks whose path `wanted` matches, with their
/// identity, ignored and hidden files included. `.git` and `.socium`
/// directories are skipped. A directory symlink is followed when it leads
/// into `.socium/scratch`, which commands may change, or after a command
/// that made or changed it: Cargo would take a crate linked into
/// `crates/` as its own.
pub(crate) fn find_matching_files(
    root: &Path,
    wanted: &dyn Fn(&str) -> bool,
    listing: Listing,
) -> Result<Listed, String> {
    #[cfg(test)]
    let timeout = tests::TIMEOUT.get().unwrap_or(LIST_TIMEOUT);
    #[cfg(not(test))]
    let timeout = LIST_TIMEOUT;
    let deadline = Instant::now() + timeout;
    let unlisted = |err: std::io::Error| format!("protected files could not be listed ({err})");
    let canonical_root = root.canonicalize().map_err(unlisted)?;
    let scratch = canonical_root.join(".socium/scratch");
    let mut visited = std::collections::HashSet::from([canonical_root]);
    let mut linked_entries = 0;
    let mut listed = Listed::default();
    let mut dirs: Vec<(PathBuf, Option<String>)> = vec![(PathBuf::new(), None)];
    while let Some((dir, through)) = dirs.pop() {
        let full = root.join(&dir);
        if matches!(listing, Listing::After(_)) && through.is_none() {
            // Commands run as the user, so a directory the user cannot
            // list still holds files they can reach.
            give_owner_listing(&full).map_err(unlisted)?;
        }
        let entries = match (std::fs::read_dir(&full), &through) {
            (Ok(entries), _) => entries,
            (Err(_), Some(link)) => {
                listed.unlisted_links.push(link.clone());
                continue;
            }
            (Err(err), None) => return Err(unlisted(err)),
        };
        for entry in entries {
            if matches!(listing, Listing::Before) && Instant::now() >= deadline {
                return Err("protected files could not be listed in time".into());
            }
            if let Some(link) = &through {
                linked_entries += 1;
                if linked_entries > MAX_LINKED_ENTRIES {
                    listed.unlisted_links.push(link.clone());
                    break;
                }
            }
            let entry = entry.map_err(unlisted)?;
            let relative = dir.join(entry.file_name());
            let file_type = entry.file_type().map_err(unlisted)?;
            if file_type.is_dir() {
                if entry.file_name() != ".git" && entry.file_name() != ".socium" {
                    dirs.push((relative, through.clone()));
                }
                continue;
            }
            let path = relative.to_string_lossy().into_owned();
            let id = file_id(&entry.metadata().map_err(unlisted)?);
            if file_type.is_symlink()
                && let Ok(target) = root.join(&relative).canonicalize()
                && target.is_dir()
            {
                listed.links.insert(path.clone(), id);
                let made =
                    matches!(listing, Listing::After(before) if before.get(&path) != Some(&id));
                if made || target.starts_with(&scratch) {
                    let link = through.clone().unwrap_or(path.clone());
                    if visited.insert(target) {
                        dirs.push((relative, Some(link)));
                    } else if made {
                        // A loop, or a second way to files already listed.
                        listed.unlisted_links.push(link);
                    }
                }
            }
            if wanted(&path) {
                listed.files.push(ListedFile {
                    path,
                    id,
                    through: through.clone(),
                });
            }
        }
    }
    listed.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(listed)
}

/// Gives the owner of `dir` read and search access if a command took it.
fn give_owner_listing(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::symlink_metadata(dir)?.permissions().mode();
    if mode & 0o500 != 0o500 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode | 0o500))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Duration;

    thread_local! {
        /// Replaces `LIST_TIMEOUT`, so a test need not fill a workspace.
        pub(crate) static TIMEOUT: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
    }
}
