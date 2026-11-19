//! User settings from `config.toml` in the Socium config directory
//! (`$XDG_CONFIG_HOME/socium/`, else `~/.config/socium/`, or the file named
//! by `SOCIUM_CONFIG_FILE`): what sandboxed commands may additionally see,
//! when tool calls need the user's approval, and which MCP servers to start
//! (ADR 12, ADR 13, ADR 20).
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
/// build: direnv, mise, nix-direnv, rust-analyzer (manifests and lock files,
/// which pick the build scripts and proc macros it runs, Cargo config,
/// toolchain files),
/// editors, CI, and Git hook managers. A change to
/// one can run code outside the sandbox later, so editing it needs approval.
pub(crate) const DEFAULT_PROTECTED: &[&str] = &[
    ".envrc",
    ".mise.toml",
    "mise.toml",
    ".tool-versions",
    "flake.nix",
    "shell.nix",
    "build.rs",
    "Cargo.toml",
    "Cargo.lock",
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
    ".pgpass",
    "auth.json",
    "credentials.json",
    "client_secret*.json",
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
    /// The `web_search` tool, which sends the model's queries to DeepSeek's
    /// paid Search API; `None` unless the user turned it on.
    pub(crate) web_search: Option<WebSearchSettings>,
    /// `[mcp.servers.<name>]`: MCP servers Socium starts on the host or
    /// connects to, in name order, the disabled ones left out.
    pub(crate) mcp_servers: Vec<McpServerSettings>,
    /// Where the settings came from, shown in errors and `/help`.
    pub(crate) source: Option<PathBuf>,
}

/// `[web_search]` in config.toml.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebSearchSettings {
    /// The Search API root, normalized without a trailing slash.
    pub(crate) base_url: String,
}

/// One `[mcp.servers.<name>]` table (ADR 20, ADR 21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpServerSettings {
    /// Matches `[A-Za-z0-9_-]{1,32}` without `__` and not ending in `_`, so
    /// `mcp__<name>__<tool>` names one server.
    pub(crate) name: String,
    pub(crate) transport: McpTransportSettings,
    /// Remote tool names that run without asking.
    pub(crate) allow: Vec<String>,
    /// How long a tool call may take before it is given up.
    pub(crate) tool_timeout: Duration,
}

/// How long an MCP tool call may take unless `tool_timeout_secs` says.
const DEFAULT_MCP_TOOL_TIMEOUT: Duration = Duration::from_secs(600);

/// How Socium reaches an MCP server.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum McpTransportSettings {
    /// `command`: a child process that speaks JSON-RPC on stdin and stdout.
    Stdio {
        /// The program, an absolute path or a name looked up on PATH, and
        /// its arguments, run with `/` as the directory.
        command: Vec<String>,
        /// Variables set for the server on top of PATH, HOME, USER and
        /// LANG, `${NAME}` already expanded from Socium's environment.
        env: BTreeMap<String, String>,
    },
    /// `url`: a remote server over Streamable HTTP.
    Http {
        /// https, or plain http on loopback.
        url: String,
        /// Sent with every request, `${NAME}` already expanded. The values
        /// are often secrets, so they are never shown.
        headers: BTreeMap<String, String>,
        /// `oauth`: a client registered with the server's authorization
        /// server ahead of time (ADR 22). A server without it still uses
        /// OAuth once `socium mcp login` stored a token for it.
        oauth: Option<McpOAuthSettings>,
    },
}

/// `oauth = { ... }` under a server with `url`.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct McpOAuthSettings {
    /// Registered ahead of time; without it, login registers a client.
    pub(crate) client_id: Option<String>,
    /// `${NAME}` already expanded. Never shown.
    pub(crate) client_secret: Option<String>,
    /// Asked for at login; empty means the server's own list, if any.
    pub(crate) scopes: Vec<String>,
    /// The login's loopback port, for a client whose redirect URI is fixed.
    pub(crate) callback_port: Option<u16>,
    /// The authorization server the client was registered with. Required
    /// with `client_secret`, so the secret goes only to that server's token
    /// endpoint, whatever the MCP server names.
    pub(crate) issuer: Option<String>,
    /// How the client was registered to prove itself; `None` picks from
    /// what the authorization server supports.
    pub(crate) token_endpoint_auth_method: Option<crate::mcp::token_store::ClientAuth>,
}

impl std::fmt::Debug for McpOAuthSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthSettings")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<hidden>"),
            )
            .field("scopes", &self.scopes)
            .field("callback_port", &self.callback_port)
            .field("issuer", &self.issuer)
            .field(
                "token_endpoint_auth_method",
                &self.token_endpoint_auth_method,
            )
            .finish()
    }
}

impl std::fmt::Debug for McpTransportSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stdio { command, env } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("env", env)
                .finish(),
            Self::Http {
                url,
                headers,
                oauth,
            } => f
                .debug_struct("Http")
                .field("url", url)
                .field("headers", &headers.keys().collect::<Vec<_>>())
                .field("oauth", oauth)
                .finish(),
        }
    }
}

/// Headers the HTTP transport sets itself.
const RESERVED_MCP_HEADERS: &[&str] = &[
    "accept",
    "content-type",
    "content-length",
    "host",
    "mcp-session-id",
    "mcp-protocol-version",
];

/// Longest server name, so `mcp__<server>__<tool>` leaves room for the tool
/// within the 64 characters providers accept.
const MAX_MCP_SERVER_NAME: usize = 32;

/// DeepSeek's API root, where `/v1/tools/search` lives.
pub(crate) const DEEPSEEK_SEARCH_BASE_URL: &str = "https://api.deepseek.com";

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
        Self(patterns.iter().map(|p| rooted(p.as_ref())).collect())
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

/// `pattern` with a leading `./` written as `/`, which anchors it at the
/// workspace root both here and in ripgrep's globs. Neither matches a
/// literal `.` component, so `./build.rs` would otherwise match nothing.
fn rooted(pattern: &str) -> String {
    let (negation, mut body) = match pattern.strip_prefix('!') {
        Some(rest) => ("!", rest),
        None => ("", pattern),
    };
    let mut stripped = false;
    while let Some(rest) = body.strip_prefix("./") {
        body = rest;
        stripped = true;
    }
    if stripped && !body.starts_with('/') {
        format!("{negation}/{body}")
    } else {
        format!("{negation}{body}")
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
    #[serde(default)]
    web_search: RawWebSearch,
    #[serde(default)]
    mcp: RawMcp,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawMcp {
    #[serde(default)]
    servers: BTreeMap<String, RawMcpServer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMcpServer {
    command: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
    url: Option<String>,
    headers: Option<BTreeMap<String, String>>,
    oauth: Option<RawMcpOAuth>,
    #[serde(default)]
    allow: Vec<String>,
    tool_timeout_secs: Option<u64>,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMcpOAuth {
    client_id: Option<String>,
    client_secret: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
    callback_port: Option<u16>,
    issuer: Option<String>,
    token_endpoint_auth_method: Option<String>,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawWebSearch {
    #[serde(default)]
    enabled: bool,
    /// For a test server or a proxy; the key is sent to it.
    base_url: Option<String>,
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

/// Directories and files that configure Socium, and the MCP token store:
/// none of them may be inside a workspace, or the model could rewrite its
/// own limits or read a token.
pub(crate) fn config_paths(lookup: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = config_dir(lookup) {
        paths.push(dir);
    }
    if let Some(store) = crate::mcp::token_store::TokenStore::from_env(lookup) {
        paths.push(store.dir().to_path_buf());
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

/// Refuses a workspace that contains Socium's own configuration or MCP
/// tokens: its file tools could then edit `config.toml`, `models.toml`, or
/// subagent types, or read a token.
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
                    "The workspace {} contains Socium's configuration ({}); the agent could change its own permissions or read MCP tokens. Start Socium in a project directory instead",
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
            Self::parse(&text, home.as_deref(), lookup).map_err(|message| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("{}: {message}", path.display()),
                )
            })?;
        settings.source = Some(path);
        Ok((settings, warnings))
    }

    /// As [`Settings::parse`] with no environment, so every `${NAME}` is
    /// missing.
    #[cfg(test)]
    pub(crate) fn from_toml(text: &str, home: Option<&str>) -> Result<(Self, Vec<String>), String> {
        Self::parse(text, home, &|_| None)
    }

    /// `env` supplies the values of `${NAME}` in MCP server variables.
    pub(crate) fn parse(
        text: &str,
        home: Option<&str>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<(Self, Vec<String>), String> {
        let raw: RawFile = toml::from_str(text).map_err(|err| toml_error(text, &err))?;
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
        let web_search = raw
            .web_search
            .enabled
            .then(|| {
                let raw_url = raw
                    .web_search
                    .base_url
                    .as_deref()
                    .unwrap_or(DEEPSEEK_SEARCH_BASE_URL);
                crate::models::parse_base_url(raw_url)
                    .map(|(base_url, _)| WebSearchSettings { base_url })
                    .map_err(|err| format!("web_search.{err}"))
            })
            .transpose()?;
        let mut mcp_servers = Vec::new();
        for (name, server) in raw.mcp.servers {
            if let Some(server) = mcp_server(name, server, env, &mut warnings)? {
                mcp_servers.push(server);
            }
        }
        Ok((
            Self {
                sandbox,
                approval,
                web_search,
                mcp_servers,
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
        for server in &self.mcp_servers {
            lines.push(match &server.transport {
                McpTransportSettings::Stdio { command, .. } => format!(
                    "MCP server {} runs on the host, outside the sandbox: {}",
                    server.name, command[0]
                ),
                McpTransportSettings::Http {
                    url,
                    headers,
                    oauth,
                } => {
                    let mut notes = Vec::new();
                    if !headers.is_empty() {
                        notes.push(format!(
                            "headers: {}",
                            headers.keys().cloned().collect::<Vec<_>>().join(", ")
                        ));
                    }
                    if let Some(client_id) = oauth.as_ref().and_then(|o| o.client_id.as_ref()) {
                        notes.push(format!("OAuth client: {client_id}"));
                    }
                    if notes.is_empty() {
                        format!("MCP server {} is remote: {url}", server.name)
                    } else {
                        format!(
                            "MCP server {} is remote: {url} ({})",
                            server.name,
                            notes.join("; ")
                        )
                    }
                }
            });
        }
        lines
    }
}

/// `[A-Za-z0-9_-]{1,32}` without `__`, so `mcp__<name>__<tool>` names one
/// server and the name is safe in a file name.
pub(crate) fn valid_mcp_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_MCP_SERVER_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        && !name.contains("__")
        && !name.ends_with('_')
}

/// A validated `[mcp.servers.<name>]`, or `None` when it is turned off or
/// names a variable that is not set (with a warning).
fn mcp_server(
    name: String,
    raw: RawMcpServer,
    env: &dyn Fn(&str) -> Option<String>,
    warnings: &mut Vec<String>,
) -> Result<Option<McpServerSettings>, String> {
    if !valid_mcp_server_name(&name) {
        return Err(format!(
            "mcp.servers.{name}: a server name must be 1 to {MAX_MCP_SERVER_NAME} letters, digits, '_' or '-', without '__' and not ending in '_'"
        ));
    }
    if raw.tool_timeout_secs == Some(0) {
        return Err(format!(
            "mcp.servers.{name}.tool_timeout_secs must be at least 1"
        ));
    }
    if let Some(tool) = raw.allow.iter().find(|tool| tool.trim().is_empty()) {
        return Err(format!(
            "mcp.servers.{name}.allow: '{tool}' is not a tool name"
        ));
    }
    let transport = match (raw.command, raw.url) {
        (Some(command), None) => {
            if raw.headers.is_some() {
                return Err(format!(
                    "mcp.servers.{name}.headers applies only to a server with url; a command server takes env"
                ));
            }
            if raw.oauth.is_some() {
                return Err(format!(
                    "mcp.servers.{name}.oauth applies only to a server with url; a command server takes env"
                ));
            }
            let Some(program) = command.first().filter(|program| !program.is_empty()) else {
                return Err(format!(
                    "mcp.servers.{name}.command must name a program and its arguments"
                ));
            };
            if program.contains('/') && !Path::new(program).is_absolute() {
                return Err(format!(
                    "mcp.servers.{name}.command: '{program}' must be an absolute path or a program name found on PATH"
                ));
            }
            let raw_env = raw.env.unwrap_or_default();
            if let Some(variable) = raw_env
                .keys()
                .find(|variable| variable.is_empty() || variable.contains(['=', '\0']))
            {
                return Err(format!(
                    "mcp.servers.{name}.env: '{variable}' is not a variable name"
                ));
            }
            if !raw.enabled {
                return Ok(None);
            }
            let Some(env) = expand_table(&name, "env", raw_env, env, warnings)? else {
                return Ok(None);
            };
            McpTransportSettings::Stdio { command, env }
        }
        (None, Some(url)) => {
            if raw.env.is_some() {
                return Err(format!(
                    "mcp.servers.{name}.env applies only to a server with command; a url server takes headers"
                ));
            }
            crate::models::check_endpoint_url("url", &url)
                .map_err(|err| format!("mcp.servers.{name}.{err}"))?;
            let raw_headers = raw.headers.unwrap_or_default();
            for header in raw_headers.keys() {
                if reqwest::header::HeaderName::from_bytes(header.as_bytes()).is_err() {
                    return Err(format!(
                        "mcp.servers.{name}.headers: '{header}' is not a header name"
                    ));
                }
                if RESERVED_MCP_HEADERS.contains(&header.to_ascii_lowercase().as_str()) {
                    return Err(format!(
                        "mcp.servers.{name}.headers: Socium sets '{header}' itself"
                    ));
                }
            }
            if !raw.enabled {
                return Ok(None);
            }
            let Some(headers) = expand_table(&name, "headers", raw_headers, env, warnings)? else {
                return Ok(None);
            };
            // Never repeat the value: it is usually a token.
            if let Some(header) = headers
                .iter()
                .find(|(_, value)| reqwest::header::HeaderValue::from_str(value).is_err())
                .map(|(header, _)| header)
            {
                return Err(format!(
                    "mcp.servers.{name}.headers.{header}: the value is not a valid header value once its variables are expanded"
                ));
            }
            let oauth = match raw.oauth {
                Some(oauth) => match oauth_settings(&name, oauth, env, warnings)? {
                    Some(oauth) => Some(oauth),
                    None => return Ok(None),
                },
                None => None,
            };
            McpTransportSettings::Http {
                url,
                headers,
                oauth,
            }
        }
        (Some(_), Some(_)) | (None, None) => {
            return Err(format!(
                "mcp.servers.{name} needs exactly one of command (a program to start) or url (a remote server)"
            ));
        }
    };
    Ok(Some(McpServerSettings {
        name,
        transport,
        allow: raw.allow,
        tool_timeout: raw
            .tool_timeout_secs
            .map_or(DEFAULT_MCP_TOOL_TIMEOUT, Duration::from_secs),
    }))
}

/// A validated `oauth` table, or `None` when `client_secret` names a
/// variable that is not set (with a warning). The secret is never repeated.
fn oauth_settings(
    name: &str,
    raw: RawMcpOAuth,
    env: &dyn Fn(&str) -> Option<String>,
    warnings: &mut Vec<String>,
) -> Result<Option<McpOAuthSettings>, String> {
    let printable =
        |value: &str| !value.trim().is_empty() && value.chars().all(|c| c.is_ascii_graphic());
    if let Some(client_id) = &raw.client_id
        && !printable(client_id)
    {
        return Err(format!(
            "mcp.servers.{name}.oauth.client_id must be printable ASCII without spaces"
        ));
    }
    if raw.client_secret.is_some() && raw.client_id.is_none() {
        return Err(format!(
            "mcp.servers.{name}.oauth.client_secret needs client_id: a secret belongs to a client registered ahead of time"
        ));
    }
    if let Some(scope) = raw.scopes.iter().find(|scope| !printable(scope)) {
        return Err(format!(
            "mcp.servers.{name}.oauth.scopes: '{scope}' is not a scope (printable ASCII without spaces)"
        ));
    }
    if raw.callback_port == Some(0) {
        return Err(format!(
            "mcp.servers.{name}.oauth.callback_port must be 1 to 65535; leave it out for any free port"
        ));
    }
    if raw.client_secret.is_some() && raw.issuer.is_none() {
        return Err(format!(
            "mcp.servers.{name}.oauth.client_secret needs issuer: the authorization server the client was registered with, so the secret is sent only there"
        ));
    }
    if let Some(issuer) = &raw.issuer {
        crate::models::check_endpoint_url("issuer", issuer)
            .map_err(|err| format!("mcp.servers.{name}.oauth.{err}"))?;
    }
    use crate::mcp::token_store::ClientAuth;
    let token_endpoint_auth_method = match raw.token_endpoint_auth_method.as_deref() {
        None => None,
        Some(_) if raw.client_secret.is_none() => {
            return Err(format!(
                "mcp.servers.{name}.oauth.token_endpoint_auth_method needs client_secret"
            ));
        }
        Some("client_secret_basic") => Some(ClientAuth::ClientSecretBasic),
        Some("client_secret_post") => Some(ClientAuth::ClientSecretPost),
        Some(other) => {
            return Err(format!(
                "mcp.servers.{name}.oauth.token_endpoint_auth_method: '{other}' is not client_secret_basic or client_secret_post"
            ));
        }
    };
    let client_secret = match raw.client_secret {
        Some(secret) => {
            let table = BTreeMap::from([("client_secret".to_string(), secret)]);
            let Some(mut expanded) = expand_table(name, "oauth", table, env, warnings)? else {
                return Ok(None);
            };
            let secret = expanded.remove("client_secret").unwrap_or_default();
            if !printable(&secret) {
                return Err(format!(
                    "mcp.servers.{name}.oauth.client_secret must be printable ASCII without spaces once its variables are expanded"
                ));
            }
            Some(secret)
        }
        None => None,
    };
    Ok(Some(McpOAuthSettings {
        client_id: raw.client_id,
        client_secret,
        scopes: raw.scopes,
        callback_port: raw.callback_port,
        issuer: raw.issuer,
        token_endpoint_auth_method,
    }))
}

/// `table` (`env` or `headers`) with each `${NAME}` expanded, or `None`
/// when a variable is not set (with a warning).
fn expand_table(
    name: &str,
    table: &str,
    raw: BTreeMap<String, String>,
    env: &dyn Fn(&str) -> Option<String>,
    warnings: &mut Vec<String>,
) -> Result<Option<BTreeMap<String, String>>, String> {
    let mut expanded = BTreeMap::new();
    for (key, value) in raw {
        match expand_variables(&value, env) {
            Ok(value) => {
                expanded.insert(key, value);
            }
            Err(Expansion::Missing(missing)) => {
                warnings.push(format!(
                    "mcp.servers.{name}.{table}.{key}: ${{{missing}}} is not set, so the {name} server is not started"
                ));
                return Ok(None);
            }
            Err(Expansion::Malformed) => {
                return Err(format!(
                    "mcp.servers.{name}.{table}.{key}: '${{' must be followed by a variable name and '}}'"
                ));
            }
        }
    }
    Ok(Some(expanded))
}

/// A TOML error by its place, with what it quotes of the file blanked
/// out: the full error repeats the line, and a line may hold a token
/// written in the file.
fn toml_error(text: &str, err: &toml::de::Error) -> String {
    let place = err
        .span()
        .and_then(|span| text.get(..span.start))
        .map(|before| {
            let line = before.matches('\n').count() + 1;
            let start = before.rsplit('\n').next().unwrap_or("");
            let column = start.chars().count() + 1;
            // The key before the value names the setting; keys are not
            // secrets, values may be.
            let key = start
                .rsplit_once('=')
                .map(|(key, _)| key)
                .and_then(|key| key.rsplit(['{', ',']).next())
                .map(|key| key.trim().trim_matches('"'))
                .filter(|key| {
                    !key.is_empty()
                        && key
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                });
            match key {
                Some(key) => format!("line {line}, column {column} ({key}): "),
                None => format!("line {line}, column {column}: "),
            }
        })
        .unwrap_or_default();
    // A type error quotes the value it found, as `string "..."`.
    let mut message = String::new();
    let mut quoted = false;
    for c in err.message().trim().chars() {
        match c {
            '"' if quoted => {
                message.push_str("…\"");
                quoted = false;
            }
            '"' => {
                message.push('"');
                quoted = true;
            }
            _ if quoted => {}
            c => message.push(c),
        }
    }
    if quoted {
        message.push('…');
    }
    format!("{place}{message}")
}

enum Expansion {
    Missing(String),
    Malformed,
}

/// `value` with each `${NAME}` replaced by the variable's value. Any other
/// `$` stays as written.
fn expand_variables(
    value: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<String, Expansion> {
    let mut expanded = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}').ok_or(Expansion::Malformed)?;
        let variable = &after[..end];
        let valid = !variable.is_empty()
            && variable
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(Expansion::Malformed);
        }
        expanded.push_str(&env(variable).ok_or_else(|| Expansion::Missing(variable.into()))?);
        rest = &after[end + 1..];
    }
    expanded.push_str(rest);
    Ok(expanded)
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
pub(crate) fn unsafe_mount(source: &Path, home: Option<&Path>) -> Option<&'static str> {
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
    fn web_search_is_off_unless_turned_on_and_keeps_keys_off_plain_http() {
        let (off, _) = Settings::from_toml("", None).unwrap();
        assert_eq!(off.web_search, None);
        let (on, _) = Settings::from_toml("[web_search]\nenabled = true", None).unwrap();
        assert_eq!(on.web_search.unwrap().base_url, DEEPSEEK_SEARCH_BASE_URL);
        let (local, _) = Settings::from_toml(
            "[web_search]\nenabled = true\nbase_url = \"http://127.0.0.1:9000/\"",
            None,
        )
        .unwrap();
        assert_eq!(local.web_search.unwrap().base_url, "http://127.0.0.1:9000");
        let err = Settings::from_toml(
            "[web_search]\nenabled = true\nbase_url = \"http://search.example\"",
            None,
        )
        .unwrap_err();
        assert!(
            err.starts_with("web_search.base_url must use https"),
            "{err}"
        );
    }

    #[test]
    fn mcp_servers_are_parsed_with_their_variables_expanded() {
        let text = r#"
[mcp.servers.github]
command = ["github-mcp-server", "stdio"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "${GITHUB_TOKEN}", MODE = "read-$only" }
allow = ["get_issue"]

[mcp.servers.time]
command = ["/usr/local/bin/uvx", "mcp-server-time"]
tool_timeout_secs = 30

[mcp.servers.off]
command = ["off"]
enabled = false
"#;
        let env = |name: &str| (name == "GITHUB_TOKEN").then(|| "ghp_x".to_string());
        let (settings, warnings) = Settings::parse(text, None, &env).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            settings.mcp_servers,
            [
                McpServerSettings {
                    name: "github".into(),
                    transport: McpTransportSettings::Stdio {
                        command: vec!["github-mcp-server".into(), "stdio".into()],
                        env: BTreeMap::from([
                            ("GITHUB_PERSONAL_ACCESS_TOKEN".into(), "ghp_x".into()),
                            ("MODE".into(), "read-$only".into()),
                        ]),
                    },
                    allow: vec!["get_issue".into()],
                    tool_timeout: Duration::from_secs(600),
                },
                McpServerSettings {
                    name: "time".into(),
                    transport: McpTransportSettings::Stdio {
                        command: vec!["/usr/local/bin/uvx".into(), "mcp-server-time".into()],
                        env: BTreeMap::new(),
                    },
                    allow: Vec::new(),
                    tool_timeout: Duration::from_secs(30),
                },
            ]
        );
        let summary = settings.summary();
        assert!(
            summary.contains(
                &"MCP server github runs on the host, outside the sandbox: github-mcp-server"
                    .to_string()
            ),
            "{summary:?}"
        );
        assert!(
            Settings::from_toml("", None)
                .unwrap()
                .0
                .mcp_servers
                .is_empty()
        );
    }

    #[test]
    fn an_mcp_server_whose_variable_is_unset_is_skipped_with_a_warning() {
        let text = "[mcp.servers.github]\ncommand = [\"gh-mcp\"]\nenv = { TOKEN = \"${GITHUB_TOKEN}\" }\n[mcp.servers.time]\ncommand = [\"time-mcp\"]";
        let (settings, warnings) = Settings::from_toml(text, None).unwrap();
        assert_eq!(settings.mcp_servers.len(), 1);
        assert_eq!(settings.mcp_servers[0].name, "time");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("${GITHUB_TOKEN} is not set")
                && warnings[0].contains("github server is not started"),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_remote_mcp_server_has_a_url_and_headers_whose_values_stay_hidden() {
        let text = r#"
[mcp.servers.github]
url = "https://api.githubcopilot.com/mcp/"
headers = { Authorization = "Bearer ${GITHUB_TOKEN}", X-Toolsets = "repos" }
allow = ["get_issue"]

[mcp.servers.local]
url = "http://127.0.0.1:8931/mcp"
"#;
        let env = |name: &str| (name == "GITHUB_TOKEN").then(|| "ghp_secret".to_string());
        let (settings, warnings) = Settings::parse(text, None, &env).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            settings.mcp_servers[0].transport,
            McpTransportSettings::Http {
                // Kept as written: the trailing slash is part of the endpoint.
                url: "https://api.githubcopilot.com/mcp/".into(),
                headers: BTreeMap::from([
                    ("Authorization".into(), "Bearer ghp_secret".into()),
                    ("X-Toolsets".into(), "repos".into()),
                ]),
                oauth: None,
            }
        );
        assert_eq!(settings.mcp_servers[0].allow, ["get_issue"]);
        assert_eq!(
            settings.summary(),
            [
                "MCP server github is remote: https://api.githubcopilot.com/mcp/ (headers: Authorization, X-Toolsets)",
                "MCP server local is remote: http://127.0.0.1:8931/mcp",
            ]
        );
        let shown = format!("{settings:?}");
        assert!(shown.contains("Authorization"), "{shown}");
        assert!(!shown.contains("ghp_secret"), "{shown}");
    }

    #[test]
    fn a_remote_mcp_server_may_name_an_oauth_client_whose_secret_stays_hidden() {
        let text = r#"
[mcp.servers.github]
url = "https://api.githubcopilot.com/mcp/"
oauth = { client_id = "Iv1.abc", client_secret = "${GITHUB_SECRET}", issuer = "https://github.com/login/oauth", token_endpoint_auth_method = "client_secret_post", scopes = ["repo", "read:org"], callback_port = 8976 }

[mcp.servers.linear]
url = "https://mcp.linear.app/mcp"
oauth = {}
"#;
        let env = |name: &str| (name == "GITHUB_SECRET").then(|| "gh-client-s3cret".to_string());
        let (settings, warnings) = Settings::parse(text, None, &env).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let McpTransportSettings::Http { oauth, .. } = &settings.mcp_servers[0].transport else {
            unreachable!("a url server");
        };
        assert_eq!(
            oauth.as_ref().unwrap(),
            &McpOAuthSettings {
                client_id: Some("Iv1.abc".into()),
                client_secret: Some("gh-client-s3cret".into()),
                scopes: vec!["repo".into(), "read:org".into()],
                callback_port: Some(8976),
                issuer: Some("https://github.com/login/oauth".into()),
                token_endpoint_auth_method: Some(
                    crate::mcp::token_store::ClientAuth::ClientSecretPost
                ),
            }
        );
        assert_eq!(
            settings.summary(),
            [
                "MCP server github is remote: https://api.githubcopilot.com/mcp/ (OAuth client: Iv1.abc)",
                "MCP server linear is remote: https://mcp.linear.app/mcp",
            ]
        );
        let shown = format!("{settings:?}");
        assert!(shown.contains("Iv1.abc"), "{shown}");
        assert!(!shown.contains("s3cret"), "{shown}");

        // An unset secret skips the server, like a header.
        let (settings, warnings) = Settings::from_toml(text, None).unwrap();
        assert_eq!(settings.mcp_servers.len(), 1);
        assert_eq!(
            warnings,
            [
                "mcp.servers.github.oauth.client_secret: ${GITHUB_SECRET} is not set, so the github server is not started"
            ]
        );
        let env = |_: &str| Some("two words s3cret".to_string());
        let err = Settings::parse(text, None, &env).unwrap_err();
        assert!(
            err.contains("oauth.client_secret must be printable"),
            "{err}"
        );
        assert!(!err.contains("s3cret"), "{err}");
    }

    #[test]
    fn a_remote_mcp_server_whose_header_variable_is_unset_is_skipped_with_a_warning() {
        let text = "[mcp.servers.github]\nurl = \"https://a.example/mcp\"\nheaders = { Authorization = \"Bearer ${GITHUB_TOKEN}\" }";
        let (settings, warnings) = Settings::from_toml(text, None).unwrap();
        assert!(settings.mcp_servers.is_empty());
        assert_eq!(
            warnings,
            [
                "mcp.servers.github.headers.Authorization: ${GITHUB_TOKEN} is not set, so the github server is not started"
            ]
        );
        // A value that is no header value is an error that does not repeat it.
        let env = |_: &str| Some("secret\nX-Injected: 1".to_string());
        let err = Settings::parse(text, None, &env).unwrap_err();
        assert!(
            err.contains(
                "mcp.servers.github.headers.Authorization: the value is not a valid header value"
            ),
            "{err}"
        );
        assert!(!err.contains("secret"), "{err}");
        let disabled = format!("{text}\nenabled = false");
        assert!(Settings::from_toml(&disabled, None).unwrap().1.is_empty());
    }

    #[test]
    fn a_malformed_config_is_reported_by_place_without_repeating_its_values() {
        for (text, place) in [
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\nheaders = { Authorization = \"Bearer s3cret-literal }\n",
                "line 3, column 53 (Authorization): ",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\nheaders = \"Authorization: Bearer s3cret-literal\"\n",
                "line 3, column 11 (headers): ",
            ),
        ] {
            let err = Settings::from_toml(text, None).unwrap_err();
            assert!(!err.contains("s3cret"), "{err}");
            assert!(err.starts_with(place), "{err}");
        }
    }

    #[test]
    fn malformed_mcp_servers_are_errors() {
        for (text, expected) in [
            (
                "[mcp.servers.\"a b\"]\ncommand = [\"x\"]",
                "mcp.servers.a b: a server name",
            ),
            (
                "[mcp.servers.a__b]\ncommand = [\"x\"]",
                "mcp.servers.a__b: a server name",
            ),
            (
                &format!("[mcp.servers.{}]\ncommand = [\"x\"]", "a".repeat(33)),
                "a server name",
            ),
            ("[mcp.servers.a_]\ncommand = [\"x\"]", "not ending in '_'"),
            ("[mcp.servers.x]\ncommand = []", "mcp.servers.x.command"),
            (
                "[mcp.servers.x]\ncommand = [\"./server\"]",
                "'./server' must be an absolute path",
            ),
            (
                "[mcp.servers.x]\ncommand = [\"x\"]\ntool_timeout_secs = 0",
                "mcp.servers.x.tool_timeout_secs",
            ),
            ("[mcp.servers.x]\nenv = {}", "needs exactly one of command"),
            (
                "[mcp.servers.x]\ncommand = [\"x\"]\nurl = \"https://a.example/mcp\"",
                "needs exactly one of command",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\nenv = {}",
                "mcp.servers.x.env applies only to a server with command",
            ),
            (
                "[mcp.servers.x]\ncommand = [\"x\"]\nheaders = { A = \"b\" }",
                "mcp.servers.x.headers applies only to a server with url",
            ),
            (
                "[mcp.servers.x]\ncommand = [\"x\"]\noauth = {}",
                "mcp.servers.x.oauth applies only to a server with url",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\noauth = { client_secret = \"s\" }",
                "mcp.servers.x.oauth.client_secret needs client_id",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\noauth = { client_id = \"a b\" }",
                "mcp.servers.x.oauth.client_id",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\noauth = { scopes = [\"a b\"] }",
                "mcp.servers.x.oauth.scopes: 'a b' is not a scope",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\noauth = { client_id = \"c\", client_secret = \"s\" }",
                "oauth.client_secret needs issuer",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\noauth = { client_id = \"c\", token_endpoint_auth_method = \"client_secret_post\" }",
                "oauth.token_endpoint_auth_method needs client_secret",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\noauth = { client_id = \"c\", client_secret = \"s\", issuer = \"https://a.example\", token_endpoint_auth_method = \"private_key_jwt\" }",
                "'private_key_jwt' is not client_secret_basic",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\noauth = { client_id = \"c\", client_secret = \"s\", issuer = \"http://a.example\" }",
                "mcp.servers.x.oauth.issuer must use https",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\noauth = { callback_port = 0 }",
                "mcp.servers.x.oauth.callback_port must be 1 to 65535",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\noauth = { callback_port = 70000 }",
                "callback_port",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\noauth = { scope = [] }",
                "unknown field",
            ),
            (
                "[mcp.servers.x]\nurl = \"http://a.example/mcp\"",
                "mcp.servers.x.url must use https (http only for localhost)",
            ),
            (
                "[mcp.servers.x]\nurl = \"ftp://127.0.0.1/mcp\"",
                "mcp.servers.x.url must use https",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp?key=1\"",
                "mcp.servers.x.url must not contain a query",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://u:p@a.example/mcp\"",
                "mcp.servers.x.url must not contain credentials",
            ),
            ("[mcp.servers.x]\nurl = \"not a url\"", "invalid url"),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\nheaders = { \"a b\" = \"c\" }",
                "mcp.servers.x.headers: 'a b' is not a header name",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\nheaders = { Mcp-Session-Id = \"c\" }",
                "Socium sets 'Mcp-Session-Id' itself",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\nheaders = { A = \"${B\" }",
                "mcp.servers.x.headers.A",
            ),
            (
                "[mcp.servers.x]\nurl = \"https://a.example/mcp\"\nheader = {}",
                "unknown field",
            ),
            (
                "[mcp.servers.x]\ncommand = [\"x\"]\nalow = []",
                "unknown field",
            ),
            ("[mcp]\nserver = {}", "unknown field"),
            (
                "[mcp.servers.x]\ncommand = [\"x\"]\nenv = { A = \"${B\" }",
                "mcp.servers.x.env.A",
            ),
            (
                "[mcp.servers.x]\ncommand = [\"x\"]\nallow = [\"\"]",
                "mcp.servers.x.allow",
            ),
        ] {
            let err = Settings::from_toml(text, None).unwrap_err();
            assert!(err.contains(expected), "{text}: {err}");
        }
    }

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
            "auth.json",
            "credentials.json",
            "config/credentials.json",
            "client_secret_1234.apps.googleusercontent.com.json",
            ".pgpass",
            "backend/auth.json",
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
        // The MCP token store counts too.
        let state = project.join("state").to_str().unwrap().to_string();
        let home_str = home.path().to_str().unwrap().to_string();
        let lookup = move |key: &str| match key {
            "HOME" => Some(home_str.clone()),
            "XDG_STATE_HOME" => Some(state.clone()),
            _ => None,
        };
        let err = ensure_config_outside(&project, &lookup).unwrap_err();
        assert!(err.to_string().contains("mcp-auth"), "{err}");
    }

    mod properties {
        use super::super::*;
        use proptest::prelude::*;

        /// The obvious recursive definition of `*` and `?` within one name.
        fn reference_glob(pattern: &[char], name: &[char]) -> bool {
            match pattern.split_first() {
                None => name.is_empty(),
                Some(('*', rest)) => {
                    (0..=name.len()).any(|skip| reference_glob(rest, &name[skip..]))
                }
                Some(('?', rest)) => !name.is_empty() && reference_glob(rest, &name[1..]),
                Some((c, rest)) => name.first() == Some(c) && reference_glob(rest, &name[1..]),
            }
        }

        fn component() -> impl Strategy<Value = String> {
            prop_oneof![
                Just(".env".to_string()),
                Just(".env.local".to_string()),
                Just(".github".to_string()),
                Just("workflows".to_string()),
                Just(".vscode".to_string()),
                Just(".cargo".to_string()),
                Just("config.toml".to_string()),
                Just("build.rs".to_string()),
                Just("key.pem".to_string()),
                "[a-c][a-c.]{0,3}",
            ]
        }

        fn relative_path() -> impl Strategy<Value = String> {
            prop::collection::vec(component(), 1..5).prop_map(|parts| parts.join("/"))
        }

        /// Patterns without `!`, drawn from the defaults and from shapes users write.
        fn positive_patterns() -> impl Strategy<Value = PathPatterns> {
            let defaults = DEFAULT_PROTECTED
                .iter()
                .chain(DEFAULT_SENSITIVE)
                .filter(|pattern| !pattern.starts_with('!'))
                .map(|pattern| pattern.to_string())
                .collect::<Vec<_>>();
            let pattern = prop_oneof![
                prop::sample::select(defaults),
                "[a-c*?][a-c.*?]{0,3}",
                ("[a-c*][a-c.*]{0,2}", "[a-c*][a-c.*]{0,2}").prop_map(|(a, b)| format!("{a}/{b}")),
                "[a-c][a-c.]{0,2}".prop_map(|dir| format!("{dir}/**")),
                "[a-c*][a-c.*]{0,2}".prop_map(|name| format!("**/{name}")),
            ];
            prop::collection::vec(pattern, 0..5).prop_map(|patterns| PathPatterns::new(&patterns))
        }

        fn absolute_path() -> impl Strategy<Value = PathBuf> {
            let part = prop::sample::select(vec![
                "proc",
                "dev",
                "tmp",
                "socium-home",
                "home",
                "user",
                "usr",
                "nix",
                "x",
            ]);
            prop::collection::vec(part, 0..4)
                .prop_map(|parts| PathBuf::from(format!("/{}", parts.join("/"))))
        }

        proptest! {
            /// The backtracking matcher protects and hides files, so it must agree with the plain definition.
            #[test]
            fn glob_segment_agrees_with_the_reference(pattern in "[ab.*?]{0,6}", name in "[ab.]{0,6}") {
                let (p, n) = (pattern.chars().collect::<Vec<_>>(), name.chars().collect::<Vec<_>>());
                prop_assert_eq!(glob_segment(&pattern, &name), reference_glob(&p, &n));
            }

            /// Spelling a path as `./p` or `p//` must not let it dodge a protected or sensitive pattern.
            #[test]
            fn path_spelling_does_not_change_a_match(patterns in positive_patterns(), path in relative_path()) {
                let expected = patterns.matches(&path);
                prop_assert_eq!(patterns.matches(&format!("./{path}")), expected);
                prop_assert_eq!(patterns.matches(&format!("{path}/")), expected);
                prop_assert_eq!(patterns.matches(&path.replace('/', "//")), expected);
            }

            /// A rooted pattern written as `./p` must protect what `p` protects, or a user's rule silently does nothing.
            #[test]
            fn pattern_spelling_does_not_change_a_match(patterns in positive_patterns(), path in relative_path()) {
                let rooted = patterns
                    .patterns()
                    .iter()
                    .map(|pattern| if pattern.contains('/') && !pattern.starts_with("**") { format!("./{pattern}") } else { pattern.clone() })
                    .collect::<Vec<_>>();
                prop_assert_eq!(PathPatterns::new(&rooted).matches(&path), patterns.matches(&path), "{:?}", rooted);
            }

            /// Everything inside a protected or sensitive directory is covered too, or a file added there escapes.
            #[test]
            fn a_match_covers_everything_below_it(patterns in positive_patterns(), path in relative_path(), child in relative_path()) {
                if patterns.matches(&path) {
                    prop_assert!(patterns.matches(&format!("{path}/{child}")), "{path}/{child}");
                }
            }

            /// An accepted read-only mount never exposes the home directory or covers a mount the sandbox makes.
            #[test]
            fn accepted_mounts_cover_no_home_and_no_sandbox_mount(source in absolute_path(), home in absolute_path()) {
                if unsafe_mount(&source, Some(&home)).is_none() {
                    for covered in [home.as_path(), Path::new("/proc"), Path::new("/dev"), Path::new("/socium-home"), Path::new("/tmp")] {
                        prop_assert!(!covered.starts_with(&source), "{} covers {}", source.display(), covered.display());
                    }
                    for owned in ["/proc", "/dev", "/socium-home"] {
                        prop_assert!(!source.starts_with(owned), "{} is inside {owned}", source.display());
                    }
                }
            }
        }
    }
}
