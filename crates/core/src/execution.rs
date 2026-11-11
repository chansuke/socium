//! Execution authority is chosen by the application, never by model arguments
//! or executable names. Every subprocess (including Git/rg helpers) uses it.
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
        })
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

pub const SANDBOX_DESCRIPTION: &str = "Linux sandbox: workspace access only plus read-only system runtimes; no network; private HOME; no host fallback";

#[cfg(all(test, target_os = "linux"))]
mod tests;
