//! What a session grant covers and which workspace files a tool must not
//! show or change without review, and the listing that finds them before
//! and after a command nobody reviewed. `approval` decides whether a call
//! asks; the tools read this to withhold files and to quarantine what such a
//! command left, so it sits below both.

use crate::settings::PathPatterns;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Most paths a command's sandbox will hide or freeze; more falls back to
/// asking, since mounting thousands of paths is slow and fragile.
const MAX_RESTRICTED_PATHS: usize = 256;

/// How long listing the workspace's protected and sensitive files may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);

/// Something the user allowed for the rest of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    /// Commands whose arguments start with these.
    Command(Vec<String>),
    /// Edits to these workspace paths.
    Edit(Vec<String>),
    /// Reads of these workspace paths.
    Read(Vec<String>),
    /// Calls to the MCP tool with this exposed name.
    McpTool(String),
}

impl Grant {
    /// What the grant covers, as the approval picker names it after
    /// "Allow".
    pub fn scope(&self) -> String {
        match self {
            Grant::Command(prefix) => format!("`{}` commands", prefix.join(" ")),
            Grant::Edit(paths) => format!("edits to {}", paths.join(", ")),
            Grant::Read(paths) => format!("reading {}", paths.join(", ")),
            Grant::McpTool(name) => format!("the MCP tool `{name}`"),
        }
    }

    fn covers(&self, other: &Grant) -> bool {
        match (self, other) {
            (Grant::Command(prefix), Grant::Command(argv)) => argv.starts_with(prefix),
            (Grant::Edit(granted), Grant::Edit(paths))
            | (Grant::Read(granted), Grant::Read(paths)) => {
                paths.iter().all(|path| granted.contains(path))
            }
            (Grant::McpTool(granted), Grant::McpTool(name)) => granted == name,
            _ => false,
        }
    }
}

/// What the sandbox withholds from a command nobody reviewed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Restrictions {
    pub(crate) hidden: Vec<PathBuf>,
    pub(crate) frozen: Vec<PathBuf>,
    /// Freezing covers only files that exist, so the protected files at the
    /// start are kept to find any the command created or replaced.
    protected: PathPatterns,
    pub(crate) before: BTreeMap<String, FileId>,
    /// Directory symlinks at the start, so the listing after the command
    /// follows the ones it made or changed.
    links: BTreeMap<String, FileId>,
}

/// A path's device, inode and change time, so a file replaced at the same
/// path shows even when the new one gets the old inode.
pub(crate) type FileId = (u64, u64, i64, i64);

pub(crate) fn file_id(metadata: &std::fs::Metadata) -> FileId {
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
struct Listed {
    /// Matching files and symlinks, by the path host tools find them at.
    files: Vec<ListedFile>,
    /// Every directory symlink.
    links: BTreeMap<String, FileId>,
    /// Followed directory symlinks with too much behind them to list.
    unlisted_links: Vec<String>,
    /// Directory symlinks followed because a pattern could match a path
    /// through them unlike the same path at their target.
    aliases: std::collections::BTreeSet<String>,
    /// Regular files with more than one link, by device and inode: a
    /// hidden file is hidden at each of its names.
    hard_links: std::collections::HashMap<(u64, u64), Vec<String>>,
}

struct ListedFile {
    path: String,
    id: FileId,
    /// The directory symlink the file was reached through: it, not the
    /// file behind it, is what the workspace holds.
    through: Option<String>,
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

/// The files a command nobody reviewed may not read (`sensitive`) or change
/// (`protected`), found with ripgrep, ignored files included. A directory
/// that matches as a whole is frozen as a whole, so new files cannot be
/// added to it either.
pub(crate) fn restrictions(
    root: &Path,
    approval: &crate::settings::ApprovalSettings,
) -> Result<Restrictions, String> {
    restrictions_before(root, approval, &Before::command())
}

fn restrictions_before(
    root: &Path,
    approval: &crate::settings::ApprovalSettings,
    before: &Before<'_>,
) -> Result<Restrictions, String> {
    let listed = find_matching_files(
        root,
        &|path| approval.protected.matches(path) || approval.sensitive.matches(path),
        Listing::Before(before),
    )?;
    if let Some(link) = listed.unlisted_links.first() {
        return Err(format!(
            "{link} links to more files than can be listed, or back to files listed already"
        ));
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|err| format!("protected files could not be listed ({err})"))?;
    let mut restrictions = Restrictions {
        protected: approval.protected.clone(),
        links: listed.links,
        ..Restrictions::default()
    };
    let mut hidden = std::collections::BTreeSet::new();
    let mut frozen_dirs: Vec<String> = Vec::new();
    for ListedFile {
        path: file,
        id,
        through,
    } in listed.files
    {
        if approval.sensitive.matches(&file) {
            if listed.aliases.contains(&file) {
                // What it leads to was listed through it and is hidden
                // file by file.
            } else if restrictions.links.contains_key(&file) {
                // An empty file cannot stand in for a directory.
                return Err(format!(
                    "{file} links to a directory whose files match approval.sensitive and cannot be hidden one by one"
                ));
            } else if through.is_some() {
                // The sandbox withholds the file at its own path, which a
                // pattern may not match when it matches the path through
                // the link.
                if let Ok(target) = root.join(&file).canonicalize()
                    && let Ok(relative) = target.strip_prefix(&canonical_root)
                {
                    hidden.insert(root.join(relative));
                }
            } else {
                hidden.insert(root.join(&file));
            }
        }
        if through.is_some() {
            if approval.protected.matches(&file) {
                restrictions.before.insert(file, id);
            }
            continue;
        }
        if approval.protected.matches(&file) {
            restrictions.before.insert(file.clone(), id);
            let target = shallowest_matching_dir(&file, &approval.protected).unwrap_or(file);
            if !frozen_dirs
                .iter()
                .any(|dir| target.starts_with(&format!("{dir}/")) || *dir == target)
            {
                frozen_dirs.push(target);
            }
        }
    }
    // Directory patterns whose directory exists at the root but holds no
    // file yet.
    for pattern in approval.protected.patterns() {
        if let Some(dir) = pattern.strip_suffix("/**")
            && !pattern.starts_with('!')
        {
            // A rooted `/dir/**` names a workspace directory, not a host one.
            let dir = dir
                .strip_prefix("**/")
                .unwrap_or(dir)
                .trim_start_matches('/');
            if !dir.contains('*')
                && root.join(dir).is_dir()
                && !frozen_dirs.iter().any(|known| known == dir)
            {
                frozen_dirs.push(dir.to_string());
            }
        }
    }
    let all = frozen_dirs.clone();
    frozen_dirs.retain(|dir| {
        !all.iter()
            .any(|other| other != dir && dir.starts_with(&format!("{other}/")))
    });
    restrictions.frozen = frozen_dirs.iter().map(|dir| root.join(dir)).collect();
    // A mount covers one name of a file, and a hard link is another name
    // of the same file.
    for path in hidden.clone() {
        use std::os::unix::fs::MetadataExt;
        if let Ok(metadata) = std::fs::metadata(&path)
            && metadata.is_file()
            && metadata.nlink() > 1
            && let Some(names) = listed.hard_links.get(&(metadata.dev(), metadata.ino()))
        {
            hidden.extend(names.iter().map(|name| root.join(name)));
        }
    }
    restrictions.hidden = hidden.into_iter().collect();
    // A hidden file under a frozen directory is mounted after it, so it
    // stays hidden.
    if restrictions.hidden.len() + restrictions.frozen.len() > MAX_RESTRICTED_PATHS {
        return Err(format!(
            "the workspace has more than {MAX_RESTRICTED_PATHS} protected or sensitive files to withhold"
        ));
    }
    Ok(restrictions)
}

/// The workspace files a sandboxed MCP server sees empty (ADR 33): those
/// `sensitive` matches now, found as for a command nobody reviewed, and
/// also in `.socium/scratch` and through the directory symlinks the server
/// can follow. `Err` when they cannot all be found by `deadline` (or
/// `LIST_TIMEOUT`, if sooner), before `stopped`, or are too many, and the
/// server must then not start.
pub(crate) fn sensitive_files(
    root: &Path,
    sensitive: &PathPatterns,
    deadline: Instant,
    stopped: &dyn Fn() -> bool,
) -> Result<Vec<PathBuf>, String> {
    let approval = crate::settings::ApprovalSettings {
        protected: PathPatterns::default(),
        sensitive: sensitive.clone(),
        ..Default::default()
    };
    let before = Before {
        deadline: Before::command().deadline.min(deadline),
        stopped,
        server: Some(sensitive),
    };
    Ok(restrictions_before(root, &approval, &before)?.hidden)
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

fn shallowest_matching_dir(file: &str, patterns: &PathPatterns) -> Option<String> {
    let parts = file.split('/').collect::<Vec<_>>();
    (1..parts.len())
        .map(|end| parts[..end].join("/"))
        .find(|dir| patterns.matches(dir))
}

/// Most entries a listing reads behind followed directory symlinks.
const MAX_LINKED_ENTRIES: usize = 10_000;

/// When a listing before a command or a sandboxed server gives up, and
/// what it lists.
struct Before<'a> {
    deadline: Instant,
    /// Whether whoever needs the listing no longer does.
    stopped: &'a dyn Fn() -> bool,
    /// For a sandboxed server (ADR 33), its `sensitive` patterns: the
    /// listing then also covers `.socium/scratch`, which the server sees,
    /// and the workspace through each directory symlink into it at whose
    /// path these could match otherwise than at its target.
    server: Option<&'a PathPatterns>,
}

impl Before<'static> {
    /// For a command that would run unasked: within `LIST_TIMEOUT`.
    fn command() -> Self {
        #[cfg(test)]
        let timeout = crate::access::tests::TIMEOUT.get().unwrap_or(LIST_TIMEOUT);
        #[cfg(not(test))]
        let timeout = LIST_TIMEOUT;
        Self {
            deadline: Instant::now() + timeout,
            stopped: &|| false,
            server: None,
        }
    }
}

/// Whether a listing runs before a command or after it.
#[derive(Clone, Copy)]
enum Listing<'a> {
    /// May give up, since the command then asks instead.
    Before(&'a Before<'a>),
    /// Must see all the command left, so it never gives up, gives back the
    /// owner access the command took from a directory, and follows the
    /// directory symlinks that are not among these from before.
    After(&'a BTreeMap<String, FileId>),
}

/// Workspace files and symlinks whose path `wanted` matches, with their
/// identity, ignored and hidden files included. `.git` directories and the
/// workspace's own `.socium` are skipped; a `.socium` deeper down is an
/// ordinary directory the sandbox does not cover. A directory symlink is
/// followed when it leads into `.socium/scratch`, which commands may
/// change, or after a command that made or changed it: Cargo would take a
/// crate linked into `crates/` as its own.
fn find_matching_files(
    root: &Path,
    wanted: &dyn Fn(&str) -> bool,
    listing: Listing,
) -> Result<Listed, String> {
    use std::os::unix::fs::MetadataExt;
    let unlisted = |err: std::io::Error| format!("protected files could not be listed ({err})");
    let canonical_root = root.canonicalize().map_err(unlisted)?;
    let scratch = canonical_root.join(".socium/scratch");
    let server = match listing {
        Listing::Before(before) => before.server,
        Listing::After(_) => None,
    };
    let given_up = || match listing {
        Listing::Before(before) if (before.stopped)() => {
            Some("the listing of protected files was cancelled")
        }
        Listing::Before(before) if Instant::now() >= before.deadline => {
            Some("protected files could not be listed in time")
        }
        _ => None,
    };
    let mut visited = std::collections::HashSet::from([canonical_root.clone()]);
    let mut linked_entries = 0;
    let mut listed = Listed::default();
    let mut dirs: Vec<(PathBuf, Option<String>)> = vec![(PathBuf::new(), None)];
    if server.is_some() && scratch.is_dir() {
        // A sandboxed server sees the scratch directory, read-only.
        dirs.push((PathBuf::from(".socium/scratch"), None));
    }
    while let Some((dir, through)) = dirs.pop() {
        if let Some(reason) = given_up() {
            return Err(reason.into());
        }
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
            if let Some(reason) = given_up() {
                return Err(reason.into());
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
                let state = dir.as_os_str().is_empty()
                    && through.is_none()
                    && entry.file_name() == ".socium";
                if entry.file_name() != ".git" && !state {
                    dirs.push((relative, through.clone()));
                }
                continue;
            }
            let path = relative.to_string_lossy().into_owned();
            let metadata = entry.metadata().map_err(unlisted)?;
            let id = file_id(&metadata);
            if file_type.is_file() && through.is_none() && metadata.nlink() > 1 {
                listed
                    .hard_links
                    .entry((metadata.dev(), metadata.ino()))
                    .or_default()
                    .push(path.clone());
            }
            if file_type.is_symlink()
                && let Ok(target) = root.join(&relative).canonicalize()
                && target.is_dir()
            {
                listed.links.insert(path.clone(), id);
                let made =
                    matches!(listing, Listing::After(before) if before.get(&path) != Some(&id));
                // Where the sandbox shows the workspace's own files, which
                // a pattern would match by another path through the link.
                let alias = server.is_some_and(|patterns| {
                    target.starts_with(&canonical_root)
                        && !target.starts_with(canonical_root.join(".git"))
                        && !target.starts_with(canonical_root.join(".socium"))
                        && target.strip_prefix(&canonical_root).is_ok_and(|relative| {
                            !patterns.alike_below(&path, &relative.to_string_lossy())
                        })
                });
                let link = through.clone().unwrap_or(path.clone());
                if alias {
                    // Each name counts, so a second link to a directory is
                    // listed again, within MAX_LINKED_ENTRIES. One back to
                    // where it is would never end.
                    if full
                        .canonicalize()
                        .is_ok_and(|here| here.starts_with(&target))
                    {
                        listed.unlisted_links.push(link);
                    } else {
                        visited.insert(target);
                        listed.aliases.insert(path.clone());
                        dirs.push((relative, Some(link)));
                    }
                } else if made || target.starts_with(&scratch) {
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
