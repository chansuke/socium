//! Execution authority is chosen by the application, never by model arguments
//! or executable names. Every subprocess (including Git/rg helpers) uses it.
use crate::settings::{ReadOnlyMount, SandboxSettings};
use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(target_os = "linux")]
mod linux;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceAccess {
    ReadOnly,
    Write,
    /// Only the explicitly approved commit tool may modify existing Git state.
    GitCommit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NetworkAccess {
    Deny,
}

pub(crate) struct ExecutionPolicy {
    pub workspace: PathBuf,
    pub access: WorkspaceAccess,
    pub network: NetworkAccess,
    /// Host directories the user listed in `sandbox.read_only`.
    pub read_only: Vec<ReadOnlyMount>,
    /// Directories the user listed in `sandbox.path`.
    pub path: Vec<PathBuf>,
    /// Variables the user listed in `sandbox.env`.
    pub env: Vec<(String, String)>,
    /// Workspace files replaced by an empty file, so a command nobody
    /// reviewed cannot read their secrets.
    pub hidden: Vec<PathBuf>,
    /// Workspace paths a command nobody reviewed cannot change.
    pub frozen: Vec<PathBuf>,
}

impl ExecutionPolicy {
    pub fn new(workspace: &Path, access: WorkspaceAccess) -> io::Result<Self> {
        let workspace = workspace.canonicalize()?;
        if !workspace.is_dir() {
            return Err(io::Error::other("sandbox workspace must be a directory"));
        }
        Ok(Self {
            workspace,
            access,
            network: NetworkAccess::Deny,
            read_only: Vec::new(),
            path: Vec::new(),
            env: Vec::new(),
            hidden: Vec::new(),
            frozen: Vec::new(),
        })
    }

    /// Adds what the user's `[sandbox]` settings expose to commands.
    pub fn with_settings(mut self, settings: &SandboxSettings) -> Self {
        self.read_only = settings.read_only.clone();
        self.path = settings.path.clone();
        self.env = settings.env.clone();
        self
    }
}

/// Whether `path` lies in a public system runtime the sandbox always mounts
/// read-only.
pub(crate) fn is_runtime_path(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::runtime_path(path)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

/// No host-execution fallback exists. Missing backends and setup errors fail
/// closed, including on platforms where we have not implemented enforcement.
pub(crate) struct SandboxExecutor;
impl SandboxExecutor {
    pub fn command(policy: &ExecutionPolicy, argv: &[String], cwd: &Path) -> io::Result<Command> {
        #[cfg(target_os = "linux")]
        {
            linux::command(policy, argv, cwd)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (policy, argv, cwd);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "OS sandbox unavailable: command execution requires Linux and bubblewrap; no host fallback",
            ))
        }
    }
}

/// Host programs Socium's own tools run, with what fails without them.
const TOOL_PROGRAMS: [(&str, &str); 2] = [
    (
        "rg",
        "ripgrep (rg) is not installed in a system directory on PATH; search_text, find_symbol and recursive list_files will fail",
    ),
    (
        "git",
        "git is not installed in a system directory on PATH; the git tools and /rewind of files will fail",
    ),
];

/// One line per program Socium's tools need but the sandbox cannot find,
/// for the startup warnings. The tools look programs up the same way, in
/// the system directories of PATH, so a warning here means they will fail.
pub(crate) fn missing_tool_programs() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        TOOL_PROGRAMS
            .iter()
            .filter(|(program, _)| !linux::finds_program(program))
            .map(|(_, warning)| warning.to_string())
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

pub const SANDBOX_DESCRIPTION: &str = "Linux sandbox: workspace access only plus read-only system runtimes; no network; private HOME; no host fallback";

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests;
