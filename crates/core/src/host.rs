//! The process environment, read in one place. Settings and model choices
//! are read through the `EnvLookup` an `AgentService` is built with, which
//! starts from [`process_env`]; the rest of this module is the host's own
//! locations.
#![expect(
    clippy::disallowed_methods,
    reason = "the one module that reads the process environment"
)]

use std::ffi::OsString;
use std::path::PathBuf;

/// One variable of the process environment, when it is set and valid UTF-8.
pub(crate) fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// The user's home directory, from `HOME`.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// `XDG_CONFIG_HOME`, when it is set.
pub(crate) fn config_home() -> Option<OsString> {
    std::env::var_os("XDG_CONFIG_HOME")
}

/// The launcher's `PATH`, when it is set.
pub(crate) fn search_path() -> Option<OsString> {
    std::env::var_os("PATH")
}

/// `$XDG_CONFIG_HOME/socium`, else `~/.config/socium`.
pub(crate) fn config_dir(lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let config_home = lookup("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            lookup("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| std::path::Path::new(&home).join(".config"))
        })?;
    Some(config_home.join("socium"))
}

/// Public runtime installations the sandbox always mounts read-only. Not
/// arbitrary PATH directories: $HOME, /etc, /run and the host's /tmp are
/// never mounted whole.
pub(crate) const RUNTIME_ROOTS: &[&str] =
    &["/usr", "/bin", "/sbin", "/lib", "/lib64", "/nix/store"];

/// Whether `path` lies in one of the [`RUNTIME_ROOTS`]; never outside
/// Linux, the only platform with a sandbox.
pub(crate) fn is_runtime_path(path: &std::path::Path) -> bool {
    cfg!(target_os = "linux") && RUNTIME_ROOTS.iter().any(|root| path.starts_with(root))
}
