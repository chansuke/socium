//! What a session grant covers and which workspace files a tool must not
//! show or change without review. `approval` decides whether a call asks;
//! the tools read this to withhold files, so it sits below both.

use std::path::{Path, PathBuf};

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

/// What the sandbox withholds from a command nobody reviewed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Restrictions {
    pub(crate) hidden: Vec<PathBuf>,
    pub(crate) frozen: Vec<PathBuf>,
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
