//! The process environment, read in one place. Settings and model choices
//! are read through the `EnvLookup` an `AgentService` is built with, which
//! starts from [`process_env`]; the rest of this module is the host's own
//! locations.

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
