//! User settings from `config.toml` in the Socium config directory
//! (`$XDG_CONFIG_HOME/socium/`, else `~/.config/socium/`, or the file named
//! by `SOCIUM_CONFIG_FILE`): what sandboxed commands may additionally see,
//! and when tool calls need the user's approval (ADR 11, ADR 12).
//!
//! The config directory must lie outside the workspace, so neither the
//! model's file tools nor its commands can change these settings.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub(crate) const CONFIG_FILE_ENV: &str = "SOCIUM_CONFIG_FILE";
pub(crate) const DEFAULT_MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_CONFIG_BYTES: u64 = 256 * 1024;
const MAX_COMMAND_TIMEOUT_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);

/// Files a host tool runs or loads on its own, without the user starting a
/// build: direnv, mise, nix-direnv, rust-analyzer (build scripts, Cargo
/// config, toolchain files), editors, CI, and Git hook managers. A change to
/// one can run code outside the sandbox later, so editing it needs approval.
pub(crate) const DEFAULT_PROTECTED: &[&str] = &[
    ".envrc",
    ".mise.toml",
    "mise.toml",
    ".tool-versions",
    "flake.nix",
    "shell.nix",
    "build.rs",
    "rust-toolchain",
    "rust-toolchain.toml",
    "**/.cargo/config",
    "**/.cargo/config.toml",
    "**/.vscode/**",
    "**/.idea/**",
    ".devcontainer/**",
    ".github/workflows/**",
    ".gitlab-ci.yml",
    ".pre-commit-config.yaml",
    ".husky/**",
];

/// Files that commonly hold credentials. Reading one needs approval, since
/// what a tool reads is sent to the model provider.
pub(crate) const DEFAULT_SENSITIVE: &[&str] = &[
    ".env",
    ".env.*",
    "!.env.example",
    "!.env.sample",
    "!.env.template",
    "!.env.dist",
    ".envrc",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
];

/// Settings for one Socium process.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub(crate) sandbox: SandboxSettings,
    pub(crate) approval: ApprovalSettings,
    /// Where the settings came from, shown in errors and `/help`.
    pub(crate) source: Option<PathBuf>,
}

/// What commands see beyond the workspace and the system runtimes.
#[derive(Debug, Clone)]
pub(crate) struct SandboxSettings {
    /// Host directories bound read-only, each at its canonical path and at
    /// the path the user wrote when that differs.
    pub(crate) read_only: Vec<ReadOnlyMount>,
    /// Directories put on PATH after the system runtime directories.
    pub(crate) path: Vec<PathBuf>,
    /// Variables set for every command.
    pub(crate) env: Vec<(String, String)>,
    /// The longest a command may run; the model asks for less.
    pub(crate) max_command_timeout: Duration,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            read_only: Vec::new(),
            path: Vec::new(),
            env: Vec::new(),
            max_command_timeout: DEFAULT_MAX_COMMAND_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadOnlyMount {
    pub(crate) source: PathBuf,
    /// The configured spelling when it is a symlink to `source`.
    pub(crate) alias: Option<PathBuf>,
}

impl SandboxSettings {
    /// Whether `path` (canonical) is inside a read-only mount.
    pub(crate) fn exposes(&self, path: &Path) -> bool {
        self.read_only
            .iter()
            .any(|mount| path.starts_with(&mount.source))
    }
}

/// When sandboxed commands run without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CommandApproval {
    /// Every command asks, unless an allow rule or a session grant covers
    /// it (Claude Code's default mode).
    #[default]
    Ask,
    /// Commands run without asking: the sandbox is the boundary (Codex's
    /// workspace-write sandbox with on-request approval, Claude Code's
    /// sandbox auto-allow). Deny rules still apply.
    Auto,
}

#[derive(Debug, Clone)]
pub(crate) struct ApprovalSettings {
    pub(crate) commands: CommandApproval,
    /// Commands that run without asking, as argument prefixes.
    pub(crate) allow: Vec<Vec<String>>,
    /// Commands that never run, as argument prefixes.
    pub(crate) deny: Vec<Vec<String>>,
    pub(crate) protected: PathPatterns,
    pub(crate) sensitive: PathPatterns,
}

impl Default for ApprovalSettings {
    fn default() -> Self {
        Self {
            commands: CommandApproval::Ask,
            allow: Vec::new(),
            deny: Vec::new(),
            protected: PathPatterns::new(DEFAULT_PROTECTED),
            sensitive: PathPatterns::new(DEFAULT_SENSITIVE),
        }
    }
}

/// Workspace path patterns. A pattern without `/` matches a file or
/// directory name at any depth; one with `/` matches the path from the
/// workspace root, where `**` stands for any number of directories and `*`
/// for part of one name. `!` excludes; the last matching pattern decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathPatterns(Vec<String>);

impl PathPatterns {
    pub(crate) fn new<S: AsRef<str>>(patterns: &[S]) -> Self {
        Self(patterns.iter().map(|p| p.as_ref().to_string()).collect())
    }

    pub(crate) fn patterns(&self) -> &[String] {
        &self.0
    }

    /// Whether `relative`, a path from the workspace root with `/`
    /// separators, matches. A path below a matching directory matches too.
    pub(crate) fn matches(&self, relative: &str) -> bool {
        let relative = relative.trim_start_matches("./");
        let mut matched = false;
        for pattern in &self.0 {
            let (negated, pattern) = match pattern.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, pattern.as_str()),
            };
            if pattern_matches(pattern, relative) {
                matched = !negated;
            }
        }
        matched
    }
}

fn pattern_matches(pattern: &str, relative: &str) -> bool {
    let path = relative
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if !pattern.contains('/') {
        // A name: any component, so files below a matching directory match.
        return path.iter().any(|name| glob_segment(pattern, name));
    }
    let parts = pattern
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    // A directory pattern also covers what is inside it.
    (0..=path.len()).any(|end| match_parts(&parts, &path[..end]))
}

fn match_parts(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| match_parts(rest, &path[skip..])),
        Some((first, rest)) => {
            !path.is_empty() && glob_segment(first, path[0]) && match_parts(rest, &path[1..])
        }
    }
}

/// `*` and `?` within one name.
fn glob_segment(pattern: &str, name: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let name = name.chars().collect::<Vec<_>>();
    let (mut p, mut n) = (0, 0);
    let mut star = None;
    let mut retry = 0;
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            retry = n;
        } else if let Some(position) = star {
            p = position + 1;
            retry += 1;
            n = retry;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    sandbox: RawSandbox,
    #[serde(default)]
    approval: RawApproval,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawSandbox {
    #[serde(default)]
    read_only: Vec<String>,
    #[serde(default)]
    path: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Seconds.
    command_timeout: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawApproval {
    commands: Option<String>,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    protected: Option<Vec<String>>,
    sensitive: Option<Vec<String>>,
}

/// Directories and files that configure Socium: none of them may be inside
/// a workspace, or the model could rewrite its own limits.
pub(crate) fn config_paths(lookup: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = config_dir(lookup) {
        paths.push(dir);
    }
    for variable in [
        CONFIG_FILE_ENV,
        crate::models::MODELS_FILE_ENV,
        crate::agent::subagent::catalog::AGENTS_DIR_ENV,
    ] {
        if let Some(path) = lookup(variable).filter(|value| !value.is_empty()) {
            paths.push(PathBuf::from(path));
        }
    }
    paths
}

/// `$XDG_CONFIG_HOME/socium`, else `~/.config/socium`.
pub(crate) fn config_dir(lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let config_home = lookup("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            lookup("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| Path::new(&home).join(".config"))
        })?;
    Some(config_home.join("socium"))
}

/// Refuses a workspace that contains Socium's own configuration: its file
/// tools could then edit `config.toml`, `models.toml`, or subagent types.
pub(crate) fn ensure_config_outside(
    workspace: &Path,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> io::Result<()> {
    let workspace = workspace.canonicalize()?;
    for path in config_paths(lookup) {
        let resolved = resolve_existing_prefix(&path);
        if resolved.starts_with(&workspace) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "The workspace {} contains Socium's configuration ({}); the agent could change its own permissions. Start Socium in a project directory instead",
                    workspace.display(),
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

/// `path` with its longest existing prefix canonicalized.
fn resolve_existing_prefix(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut resolved = canonical;
            for part in rest.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        match (current.file_name(), current.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                current = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

impl Settings {
    /// Reads the settings file, or the defaults when there is none. A
    /// malformed file is an error; a directory that does not exist on this
    /// machine is skipped with a warning.
    pub(crate) fn load(lookup: &dyn Fn(&str) -> Option<String>) -> io::Result<(Self, Vec<String>)> {
        let explicit = lookup(CONFIG_FILE_ENV).filter(|value| !value.is_empty());
        let Some(path) = explicit
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| config_dir(lookup).map(|dir| dir.join("config.toml")))
        else {
            return Ok((Self::default(), Vec::new()));
        };
        let text = match fs::metadata(&path) {
            Ok(metadata) if metadata.len() > MAX_CONFIG_BYTES => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("{} is larger than 256 KiB", path.display()),
                ));
            }
            Ok(_) => fs::read_to_string(&path).map_err(|err| {
                Error::new(err.kind(), format!("Cannot read {}: {err}", path.display()))
            })?,
            Err(err) if err.kind() == ErrorKind::NotFound && explicit.is_none() => {
                return Ok((Self::default(), Vec::new()));
            }
            Err(err) => {
                return Err(Error::new(
                    err.kind(),
                    format!("Cannot read {}: {err}", path.display()),
                ));
            }
        };
        let home = lookup("HOME").filter(|home| !home.is_empty());
        let (mut settings, warnings) =
            Self::from_toml(&text, home.as_deref()).map_err(|message| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("{}: {message}", path.display()),
                )
            })?;
        settings.source = Some(path);
        Ok((settings, warnings))
    }

    pub(crate) fn from_toml(text: &str, home: Option<&str>) -> Result<(Self, Vec<String>), String> {
        let raw: RawFile = toml::from_str(text).map_err(|err| err.to_string())?;
        let mut warnings = Vec::new();
        let expand = |value: &str| -> Result<PathBuf, String> {
            let path = match value.strip_prefix("~/") {
                Some(rest) => Path::new(home.ok_or("~ needs HOME to be set")?).join(rest),
                None if value == "~" => PathBuf::from(home.ok_or("~ needs HOME to be set")?),
                None => PathBuf::from(value),
            };
            if !path.is_absolute() {
                return Err(format!(
                    "'{value}' must be an absolute path or start with ~/"
                ));
            }
            Ok(path)
        };
        let home_dir = home.and_then(|home| Path::new(home).canonicalize().ok());

        let mut read_only = Vec::new();
        for value in &raw.sandbox.read_only {
            let configured = expand(value)?;
            let source = match configured.canonicalize() {
                Ok(source) => source,
                Err(_) => {
                    warnings.push(format!(
                        "sandbox.read_only: '{value}' does not exist on this machine; skipped"
                    ));
                    continue;
                }
            };
            if let Some(reason) = unsafe_mount(&source, home_dir.as_deref()) {
                return Err(format!("sandbox.read_only: '{value}' {reason}"));
            }
            let alias = (normalize(&configured) != source).then(|| normalize(&configured));
            let mount = ReadOnlyMount { source, alias };
            if !read_only.contains(&mount) {
                read_only.push(mount);
            }
        }
        let mut sandbox = SandboxSettings {
            read_only,
            ..SandboxSettings::default()
        };
        for value in &raw.sandbox.path {
            let configured = expand(value)?;
            match configured.canonicalize() {
                Ok(dir) if dir.is_dir() => {
                    if !sandbox.exposes(&dir) && !crate::execution::is_runtime_path(&dir) {
                        warnings.push(format!(
                            "sandbox.path: '{value}' is not inside a sandbox.read_only directory or a system runtime, so commands cannot see it; skipped"
                        ));
                        continue;
                    }
                    if !sandbox.path.contains(&dir) {
                        sandbox.path.push(dir);
                    }
                }
                _ => warnings.push(format!(
                    "sandbox.path: '{value}' is not a directory on this machine; skipped"
                )),
            }
        }
        for (name, value) in &raw.sandbox.env {
            if let Some(reason) = reserved_variable(name) {
                return Err(format!("sandbox.env: {name} {reason}"));
            }
            let value = match value.strip_prefix("~/") {
                Some(_) => expand(value)?.to_string_lossy().into_owned(),
                None => value.clone(),
            };
            sandbox.env.push((name.clone(), value));
        }
        if let Some(seconds) = raw.sandbox.command_timeout {
            let timeout = Duration::from_secs(seconds);
            if seconds == 0 || timeout > MAX_COMMAND_TIMEOUT_LIMIT {
                return Err("sandbox.command_timeout must be between 1 and 86400 seconds".into());
            }
            sandbox.max_command_timeout = timeout;
        }

        let parse_rules = |field: &str, rules: &[String]| -> Result<Vec<Vec<String>>, String> {
            rules
                .iter()
                .map(|rule| {
                    let tokens = rule
                        .split_whitespace()
                        .map(str::to_string)
                        .collect::<Vec<_>>();
                    if tokens.is_empty() {
                        Err(format!("approval.{field}: a rule cannot be empty"))
                    } else {
                        Ok(tokens)
                    }
                })
                .collect()
        };
        let commands = match raw.approval.commands.as_deref() {
            None | Some("ask") => CommandApproval::Ask,
            Some("auto") => CommandApproval::Auto,
            Some(other) => {
                return Err(format!(
                    "approval.commands must be \"ask\" or \"auto\"; got '{other}'"
                ));
            }
        };
        let approval = ApprovalSettings {
            commands,
            allow: parse_rules("allow", &raw.approval.allow)?,
            deny: parse_rules("deny", &raw.approval.deny)?,
            protected: raw
                .approval
                .protected
                .map(|patterns| PathPatterns::new(&patterns))
                .unwrap_or_else(|| PathPatterns::new(DEFAULT_PROTECTED)),
            sensitive: raw
                .approval
                .sensitive
                .map(|patterns| PathPatterns::new(&patterns))
                .unwrap_or_else(|| PathPatterns::new(DEFAULT_SENSITIVE)),
        };
        Ok((
            Self {
                sandbox,
                approval,
                source: None,
            },
            warnings,
        ))
    }

    /// One line per non-default setting, for the startup notice.
    pub(crate) fn summary(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for mount in &self.sandbox.read_only {
            lines.push(format!(
                "read-only for commands: {}",
                mount.source.display()
            ));
        }
        if self.approval.commands == CommandApproval::Auto {
            lines
                .push("approval.commands = \"auto\": sandboxed commands run without asking".into());
        }
        lines
    }
}

/// `.` and `..` resolved without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Why `source` must not be mounted: the whole filesystem, the home
/// directory or an ancestor of it (which would expose `~/.ssh` and every
/// credential), or a path the sandbox itself provides.
fn unsafe_mount(source: &Path, home: Option<&Path>) -> Option<&'static str> {
    if source == Path::new("/") {
        return Some("would expose the whole filesystem");
    }
    if home.is_some_and(|home| home.starts_with(source)) {
        return Some(
            "is the home directory or contains it; list the directories commands need instead",
        );
    }
    for reserved in ["/proc", "/dev", "/socium-home"] {
        let reserved = Path::new(reserved);
        if source.starts_with(reserved) || reserved.starts_with(source) {
            return Some("overlaps a path the sandbox provides");
        }
    }
    if Path::new("/tmp").starts_with(source) {
        return Some("is /tmp or contains it, which the sandbox replaces");
    }
    None
}

/// Variables the sandbox sets itself, or that would undo the Git isolation.
fn reserved_variable(name: &str) -> Option<&'static str> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    if !valid {
        return Some("must be an upper-case variable name");
    }
    if matches!(name, "PATH" | "HOME" | "TMPDIR") {
        return Some("is set by the sandbox; use sandbox.path for PATH");
    }
    if name.starts_with("GIT_") {
        return Some("would change Git's configuration isolation");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_file_is_valid() {
        let home = tempfile::tempdir().unwrap();
        for dir in [".cargo/registry", ".cargo/git", ".rustup"] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }
        let (settings, warnings) = Settings::from_toml(
            include_str!("../../../config.toml.example"),
            Some(home.path().to_str().unwrap()),
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(settings.sandbox.read_only.len(), 3);
        assert_eq!(
            settings.sandbox.max_command_timeout,
            Duration::from_secs(600)
        );
        assert_eq!(settings.approval.allow.len(), 3);
    }

    #[test]
    fn name_patterns_match_at_any_depth_and_last_match_wins() {
        let patterns = PathPatterns::new(DEFAULT_SENSITIVE);
        for path in [
            ".env",
            "app/.env",
            ".env.local",
            "certs/server.pem",
            ".envrc",
        ] {
            assert!(patterns.matches(path), "{path}");
        }
        for path in [".env.example", "app/.env.sample", "src/env.rs", "README.md"] {
            assert!(!patterns.matches(path), "{path}");
        }
    }

    #[test]
    fn path_patterns_anchor_at_the_root_and_cover_directories() {
        let patterns = PathPatterns::new(DEFAULT_PROTECTED);
        for path in [
            "build.rs",
            "crates/core/build.rs",
            ".github/workflows/ci.yaml",
            ".github/workflows",
            ".cargo/config.toml",
            "crates/x/.cargo/config.toml",
            ".vscode/tasks.json",
            "sub/.vscode/settings.json",
        ] {
            assert!(patterns.matches(path), "{path}");
        }
        for path in [
            "docs/.github/workflows/x",
            "src/main.rs",
            "Cargo.toml",
            ".github/CODEOWNERS",
        ] {
            assert!(!patterns.matches(path), "{path}");
        }
    }

    #[test]
    fn a_config_file_sets_mounts_rules_and_limits() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".cargo/registry")).unwrap();
        std::fs::create_dir_all(home.path().join(".cargo/bin")).unwrap();
        let text = r#"
[sandbox]
read_only = ["~/.cargo", "~/missing"]
path = ["~/.cargo/bin", "~/elsewhere"]
command_timeout = 600
[sandbox.env]
CARGO_HOME = "~/.cargo"

[approval]
commands = "auto"
allow = ["cargo test"]
deny = ["git  push"]
"#;
        let home_str = home.path().to_str().unwrap();
        let (settings, warnings) = Settings::from_toml(text, Some(home_str)).unwrap();
        let cargo = home.path().join(".cargo").canonicalize().unwrap();
        assert_eq!(settings.sandbox.read_only[0].source, cargo);
        assert_eq!(settings.sandbox.path, [cargo.join("bin")]);
        assert_eq!(
            settings.sandbox.env,
            [("CARGO_HOME".to_string(), format!("{home_str}/.cargo"))]
        );
        assert_eq!(
            settings.sandbox.max_command_timeout,
            Duration::from_secs(600)
        );
        assert_eq!(settings.approval.commands, CommandApproval::Auto);
        assert_eq!(settings.approval.allow, [vec!["cargo", "test"]]);
        assert_eq!(settings.approval.deny, [vec!["git", "push"]]);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("~/missing"));
        assert!(warnings[1].contains("~/elsewhere"));
    }

    #[test]
    fn dangerous_or_malformed_settings_are_errors() {
        let home = tempfile::tempdir().unwrap();
        let home_str = home.path().to_str().unwrap();
        for (text, expected) in [
            ("[sandbox]\nread_only = [\"/\"]", "whole filesystem"),
            ("[sandbox]\nread_only = [\"~\"]", "home directory"),
            ("[sandbox]\nread_only = [\"relative\"]", "absolute"),
            ("[sandbox.env]\nPATH = \"/x\"", "set by the sandbox"),
            ("[sandbox.env]\nGIT_DIR = \"/x\"", "Git"),
            ("[sandbox]\ncommand_timeout = 0", "between"),
            ("[approval]\ncommands = \"never\"", "ask"),
            ("[approval]\nallow = [\" \"]", "empty"),
            ("[sandbox]\nread_olny = []", "unknown field"),
        ] {
            let err = Settings::from_toml(text, Some(home_str)).unwrap_err();
            assert!(err.contains(expected), "{text}: {err}");
        }
    }

    #[test]
    fn a_workspace_holding_the_config_directory_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let home_str = home.path().to_str().unwrap().to_string();
        let lookup = move |key: &str| (key == "HOME").then(|| home_str.clone());
        assert!(ensure_config_outside(home.path(), &lookup).is_err());
        let project = home.path().join("project");
        std::fs::create_dir(&project).unwrap();
        ensure_config_outside(&project, &lookup).unwrap();
    }
}
