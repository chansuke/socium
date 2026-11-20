use crate::AgentProgress;
use crate::conversation::ToolDefinition;
use crate::tool_error::ToolError;
use crate::tools::input::{
    optional_bounded_usize_field, optional_string_field, require_object_input, require_string_field,
};
use crate::tools::{Tool, ToolContext};
use crate::workspace::resolve_workspace_path;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) struct GitBranchTool;

impl Tool for GitBranchTool {
    fn name(&self) -> &'static str {
        "git_branch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_branch".to_string(),
            description: "Show the current Git branch for the workspace repository.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        _on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        git_branch_from_workspace(ctx.workspace_root, input)
    }
}

pub(crate) struct GitStatusTool;

impl Tool for GitStatusTool {
    fn name(&self) -> &'static str {
        "git_status"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_status".to_string(),
            description:
                "Show Git status for the workspace repository, including branch and dirty files."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional relative path to limit status output to part of the workspace."
                    }
                },
                "additionalProperties": false
            }),
        }
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        _on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        git_status_from_workspace(ctx.workspace_root, input)
    }
}

pub(crate) struct GitDiffTool;

impl Tool for GitDiffTool {
    fn name(&self) -> &'static str {
        "git_diff"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_diff".to_string(),
            description:
                "Show the current Git diff for the workspace repository or an optional relative path."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional relative path to limit the diff."
                    }
                },
                "additionalProperties": false
            }),
        }
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        _on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        git_diff_withholding(ctx.workspace_root, input, &ctx.settings.approval.sensitive)
    }
}

pub(crate) struct GitLogTool;

impl Tool for GitLogTool {
    fn name(&self) -> &'static str {
        "git_log"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_log".to_string(),
            description: "Show recent Git commit history for the workspace repository.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "max_count": {
                        "type": "integer",
                        "description": "Optional number of commits to return."
                    }
                },
                "additionalProperties": false
            }),
        }
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        _on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        git_log_from_workspace(ctx.workspace_root, input)
    }
}

pub(crate) struct GitCommitTool;

impl Tool for GitCommitTool {
    fn name(&self) -> &'static str {
        "git_commit"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_commit".to_string(),
            description: "Commit staged changes by default, or only explicitly listed tracked files, after human approval. Unrelated staged, unstaged and untracked changes are preserved. Files already staged are rejected to protect partial staging."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "Commit message to use."
                    },
                    "paths": {"type":"array", "items":{"type":"string"}, "minItems":1, "description":"Exact file paths to commit; no directories, wildcards or implicit all-files scope."}
                },
                "required": ["message"],
                "additionalProperties": false
            }),
        }
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        let mut forward = |progress| on_progress(progress);
        git_commit_controlled(ctx.workspace_root, input, ctx.control, &mut forward)
    }
}

struct GitPathInput<'a> {
    path: Option<&'a str>,
}

impl<'a> GitPathInput<'a> {
    fn parse(input: &'a Value, tool: &str, field_context: &str) -> Result<Self, ToolError> {
        let input_object = require_object_input(input, tool)?;
        let path = optional_string_field(input_object, "path", field_context)?;
        Ok(Self { path })
    }
}

struct GitLogInput {
    max_count: usize,
}

impl GitLogInput {
    fn parse(input: &Value) -> Result<Self, ToolError> {
        let input_object = require_object_input(input, "git_log")?;
        let max_count =
            optional_bounded_usize_field(input_object, "max_count", "git_log", 10, 100)?;
        Ok(Self { max_count })
    }
}

pub(crate) fn git_branch_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    require_object_input(input, "git_branch")?;
    let output = run_git_capture(workspace_root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = stdout_to_string(&output)?.trim().to_string();
    Ok(format!("Branch: {branch}"))
}

pub(crate) fn git_status_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    let GitPathInput { path } = GitPathInput::parse(input, "git_status", "git_status path")?;
    let mut args = vec![
        "status".to_string(),
        "--short".to_string(),
        "--branch".to_string(),
    ];
    if let Some(path) = path {
        args.push("--".to_string());
        args.push(git_relative_path(workspace_root, path)?);
    }
    let output = run_git_capture(workspace_root, &args)?;
    let stdout = stdout_to_string(&output)?;
    let mut lines = stdout.lines();
    let branch_line = lines.next().unwrap_or("## HEAD");
    let branch = branch_line.trim_start_matches("## ").trim();
    let mut result = vec![format!("Branch: {branch}")];
    if let Some(path) = path {
        result.push(format!("Path: {path}"));
    }
    let status_lines = lines.collect::<Vec<_>>();
    if status_lines.is_empty() {
        result.push("(clean)".to_string());
    } else {
        result.extend(status_lines.into_iter().map(ToString::to_string));
    }
    Ok(result.join("\n"))
}

#[cfg(test)]
pub(crate) fn git_diff_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    git_diff_withholding(
        workspace_root,
        input,
        &crate::settings::Settings::default().approval.sensitive,
    )
}

/// `git diff`, without the changes to files that may hold credentials
/// unless the path names one.
fn git_diff_withholding(
    workspace_root: &Path,
    input: &Value,
    sensitive: &crate::settings::PathPatterns,
) -> Result<String, ToolError> {
    let GitPathInput { path } = GitPathInput::parse(input, "git_diff", "git_diff path")?;
    let mut pathspec = vec!["--".to_string()];
    if let Some(path) = path {
        pathspec.push(git_relative_path(workspace_root, path)?);
    }
    let changed = run_git_capture(
        workspace_root,
        &[
            ["diff", "--name-only", "--relative", "-z"]
                .map(String::from)
                .as_slice(),
            &pathspec,
        ]
        .concat(),
    )?;
    let named = path
        .and_then(|path| crate::approval::relative_target(workspace_root, path))
        .unwrap_or_default();
    let (withheld, shown): (Vec<String>, Vec<String>) = stdout_to_string(&changed)?
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .partition(|name| crate::approval::withholds(sensitive, name, &named));
    // Pathspecs are literal here, so the files to show are named one by one
    // rather than the others excluded.
    if !withheld.is_empty() {
        pathspec.truncate(1);
        pathspec.extend(shown.iter().cloned());
    }
    let mut args = vec![
        "diff".to_string(),
        "--no-ext-diff".into(),
        "--no-textconv".into(),
    ];
    args.extend(pathspec);
    let stdout = if !withheld.is_empty() && shown.is_empty() {
        String::new()
    } else {
        stdout_to_string(&run_git_capture(workspace_root, &args)?)?
    };
    let mut result = Vec::new();
    if let Some(path) = path {
        result.push(format!("Path: {path}"));
    }
    if stdout.trim().is_empty() {
        result.push("(no diff)".to_string());
    } else {
        result.push(stdout);
    }
    if !withheld.is_empty() {
        result.push(crate::approval::withheld_note(&withheld));
    }
    Ok(result.join("\n"))
}

pub(crate) fn git_log_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    let GitLogInput { max_count } = GitLogInput::parse(input)?;
    let output = run_git_capture(
        workspace_root,
        &[
            "log".to_string(),
            "--oneline".to_string(),
            format!("-n{max_count}"),
        ],
    )?;
    let stdout = stdout_to_string(&output)?;
    let mut result = vec![format!("Count: {max_count}")];
    if stdout.trim().is_empty() {
        result.push("(no commits)".to_string());
    } else {
        result.push(stdout);
    }
    Ok(result.join("\n"))
}

fn commit_paths(root: &Path, input: &Value) -> Result<Option<Vec<String>>, ToolError> {
    let object = require_object_input(input, "git_commit")?;
    if !object.contains_key("paths") {
        return Ok(None);
    }
    let paths = crate::tools::input::require_string_array_field(object, "paths", "git_commit")?;
    let canonical = root.canonicalize().map_err(|e| e.to_string())?;
    paths
        .iter()
        .map(|path| commit_pathspec(&canonical, path))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// The Git pathspec for one requested path. Git names the entry itself, so
/// the last component is never followed: a symlink is committed as a link,
/// not as the file it points to. Parent directories are resolved like any
/// other tool path, which keeps the entry inside the workspace and outside
/// Git and Socium state.
fn commit_pathspec(canonical_root: &Path, requested: &str) -> Result<String, ToolError> {
    use std::path::Component;
    let path = Path::new(requested);
    let Some(Component::Normal(name)) = path.components().next_back() else {
        return Err("git_commit accepts files only".into());
    };
    let directory = match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => crate::workspace::resolve_workspace_path_for_create(
            canonical_root,
            &parent.to_string_lossy(),
        )?,
        None => canonical_root.to_path_buf(),
    };
    if directory.exists() && !directory.is_dir() {
        return Err(format!("Parent is not a directory: '{requested}'").into());
    }
    let entry = directory.join(name);
    match std::fs::symlink_metadata(&entry) {
        Ok(metadata) if metadata.is_dir() => return Err("git_commit accepts files only".into()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Failed to inspect path '{requested}': {e}").into()),
    }
    let relative = entry
        .strip_prefix(canonical_root)
        .map_err(|e| e.to_string())?;
    if matches!(
        relative.components().next(),
        Some(Component::Normal(first)) if first == ".git" || first == ".socium"
    ) {
        return Err("Cannot commit agent or Git state".into());
    }
    Ok(relative.to_string_lossy().into_owned())
}

/// The listed paths Git does not track yet: files the agent (or the user)
/// created that a scoped commit adds.
fn untracked_paths(root: &Path, paths: &[String]) -> Result<Vec<String>, ToolError> {
    let mut args = vec![
        "--literal-pathspecs".to_string(),
        "ls-files".into(),
        "-z".into(),
        "--".into(),
    ];
    args.extend(paths.iter().cloned());
    let tracked = run_git_capture(root, &args)?.stdout;
    let tracked = tracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect::<Vec<_>>();
    let mut untracked = Vec::new();
    for path in paths {
        if tracked.contains(path) {
            continue;
        }
        if std::fs::symlink_metadata(root.join(path)).is_err() {
            return Err(format!("'{path}' is neither tracked nor present").into());
        }
        untracked.push(path.clone());
    }
    Ok(untracked)
}

/// A new file as the reviewer sees it.
fn new_file_review(root: &Path, path: &str) -> String {
    const SHOWN: u64 = 64 * 1024;
    let full = root.join(path);
    match std::fs::symlink_metadata(&full) {
        Ok(metadata) if metadata.file_type().is_symlink() => format!(
            "New symlink: {path} -> {}\n",
            std::fs::read_link(&full).map_or_else(|e| e.to_string(), |t| t.display().to_string())
        ),
        Ok(metadata) => {
            let mut bytes = Vec::new();
            let read = std::fs::File::open(&full).and_then(|file| {
                std::io::Read::read_to_end(&mut std::io::Read::take(file, SHOWN), &mut bytes)
            });
            match (read, String::from_utf8(bytes)) {
                (Ok(_), Ok(text)) => {
                    let mut shown = format!("New file: {path} ({} bytes)\n", metadata.len());
                    for line in text.lines() {
                        shown.push('+');
                        shown.push_str(line);
                        shown.push('\n');
                    }
                    if metadata.len() > SHOWN {
                        shown.push_str("(only the first 64 KiB shown)\n");
                    }
                    shown
                }
                _ => format!(
                    "New file: {path} ({} bytes, not UTF-8 text)\n",
                    metadata.len()
                ),
            }
        }
        Err(e) => format!("New file: {path} ({e})\n"),
    }
}

/// Author and committer for the commit. The repository's own setting wins;
/// otherwise the user's global Git identity is passed in, since the sandbox
/// does not load global configuration.
fn commit_identity(root: &Path) -> Result<Vec<(String, String)>, ToolError> {
    let local = |key: &str| -> Result<String, ToolError> {
        Ok(stdout_to_string(&run_git_capture(
            root,
            &["config", "--default", "", "--get", key],
        )?)?
        .trim()
        .to_string())
    };
    let (name, email) = (local("user.name")?, local("user.email")?);
    if !name.is_empty() && !email.is_empty() {
        return Ok(Vec::new());
    }
    let name = if name.is_empty() {
        host_git_identity("user.name")
    } else {
        Some(name)
    };
    let email = if email.is_empty() {
        host_git_identity("user.email")
    } else {
        Some(email)
    };
    match (name, email) {
        (Some(name), Some(email)) => Ok(["AUTHOR", "COMMITTER"]
            .iter()
            .flat_map(|role| {
                [
                    (format!("GIT_{role}_NAME"), name.clone()),
                    (format!("GIT_{role}_EMAIL"), email.clone()),
                ]
            })
            .collect()),
        _ => Err("No Git identity: set user.name and user.email with `git config` (globally or in this repository) before committing".into()),
    }
}

/// A value from the user's global Git configuration, read by the host's Git
/// outside any repository, so nothing from the workspace is loaded and no
/// helper runs.
fn host_git_identity(key: &str) -> Option<String> {
    let home = crate::host::home_dir()?;
    host_git_config(key, home.as_os_str(), crate::host::config_home().as_deref())
}

fn host_git_config(
    key: &str,
    home: &std::ffi::OsStr,
    config_home: Option<&std::ffi::OsStr>,
) -> Option<String> {
    let git = std::env::split_paths(&crate::host::search_path()?)
        .map(|dir| dir.join("git"))
        .find(|path| path.is_absolute() && path.is_file())?;
    let mut command = std::process::Command::new(git);
    command
        .args(["config", "--global", "--includes", "--get", key])
        .env_clear()
        .env("HOME", home)
        .current_dir("/")
        .stdin(std::process::Stdio::null());
    if let Some(config_home) = config_home {
        command.env("XDG_CONFIG_HOME", config_home);
    }
    let output = command.output().ok()?;
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !value.is_empty() && !value.chars().any(char::is_control))
        .then_some(value)
}

pub(crate) fn commit_review(root: &Path, input: &Value) -> Result<String, ToolError> {
    let paths = commit_paths(root, input)?;
    let mut diff_args = vec![
        "--literal-pathspecs".to_string(),
        "diff".into(),
        "--no-ext-diff".into(),
        "--no-textconv".into(),
        "--binary".into(),
    ];
    let mut new_files = String::new();
    if let Some(paths) = paths {
        let untracked = untracked_paths(root, &paths)?;
        for path in &untracked {
            new_files.push_str(&new_file_review(root, path));
        }
        diff_args.push("--".into());
        diff_args.extend(paths.into_iter().filter(|path| !untracked.contains(path)));
    } else {
        diff_args.push("--cached".into());
    }
    let mut diff = if diff_args.last().is_some_and(|last| last == "--") {
        String::new()
    } else {
        stdout_to_string(&run_git_capture(root, &diff_args)?)?
    };
    diff.push_str(&new_files);
    if diff.trim().is_empty() {
        return Err("No changes in requested commit scope; name the files in paths, or stage them before a staged-only commit".into());
    }
    let identity = match commit_identity(root)? {
        env if env.is_empty() => "repository configuration".to_string(),
        env => format!(
            "{} <{}> from your global Git configuration",
            env[0].1, env[1].1
        ),
    };
    let index = stdout_to_string(&run_git_capture(
        root,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
        ],
    )?)?;
    let head = stdout_to_string(&run_git_capture(
        root,
        &["status", "--porcelain=v2", "--branch"],
    )?)?;
    Ok(format!(
        "Author: {identity}\nRepository state:\n{head}\nRequested diff:\n{diff}\nExisting staged diff:\n{index}"
    ))
}

fn git_commit_controlled<F>(
    workspace_root: &Path,
    input: &Value,
    control: &crate::TaskControl,
    _on_progress: &mut F,
) -> Result<String, ToolError>
where
    F: FnMut(AgentProgress),
{
    let object = require_object_input(input, "git_commit")?;
    let message = require_string_field(object, "message", "git_commit")?;
    if message.trim().is_empty() {
        return Err("Empty commit message".into());
    }
    let paths = commit_paths(workspace_root, input)?;
    let mut untracked = Vec::new();
    let mut args = vec![
        "git".into(),
        "--literal-pathspecs".into(),
        "commit".into(),
        "-m".into(),
        message.into(),
    ];
    if let Some(paths) = paths {
        let mut staged_args = vec![
            "--literal-pathspecs".to_string(),
            "diff".into(),
            "--cached".into(),
            "--name-only".into(),
            "--".into(),
        ];
        staged_args.extend(paths.clone());
        if !run_git_capture(workspace_root, &staged_args)?
            .stdout
            .is_empty()
        {
            return Err("Selected paths already have staged changes; use staged-only mode or resolve partial staging first".into());
        }
        untracked = untracked_paths(workspace_root, &paths)?;
        args.extend(["--only".into(), "--".into()]);
        args.extend(paths);
    }
    // --only preserves unrelated index entries and excludes unselected files.
    // A new file the user named in the approved request is marked with
    // intent-to-add so that --only can take it; if the commit fails, the
    // mark is removed and the file is untracked again.
    let mut policy = crate::execution::ExecutionPolicy::new(
        workspace_root,
        crate::execution::WorkspaceAccess::GitCommit,
    )
    .map_err(|e| e.to_string())?;
    policy.env = commit_identity(workspace_root)?;
    let run = |args: &[String]| {
        crate::process::run_observed(
            args,
            workspace_root,
            std::time::Duration::from_secs(60),
            control,
            &mut |_, _| Ok(()),
            &policy,
        )
        .map_err(|e| e.to_string())
    };
    let git = |rest: &[&str], paths: &[String]| {
        let mut args = vec!["git".to_string(), "--literal-pathspecs".into()];
        args.extend(rest.iter().map(|s| s.to_string()));
        args.push("--".into());
        args.extend(paths.iter().cloned());
        args
    };
    if !untracked.is_empty() {
        let added = run(&git(&["add", "--intent-to-add"], &untracked))?;
        if !added.status.success() {
            return Err(format!(
                "Could not add the new files: {}",
                String::from_utf8_lossy(&added.stderr)
            )
            .into());
        }
    }
    let output = run(&args)?;
    if output.cancelled || output.timed_out || !output.status.success() {
        if !untracked.is_empty() && !output.cancelled {
            let _ = run(&git(&["reset", "-q"], &untracked));
        }
        return Err(format!("Git commit incomplete (cancelled={}, timed_out={}): {}. Inspect HEAD and index before retrying.",output.cancelled,output.timed_out,String::from_utf8_lossy(&output.stderr)).into());
    }
    let head = run_git_capture(workspace_root, &["rev-parse", "--short", "HEAD"])?;
    Ok(format!(
        "Commit: {}\nMessage: {message}",
        stdout_to_string(&head)?.trim()
    ))
}

fn run_git_capture<S>(workspace_root: &Path, args: &[S]) -> Result<std::process::Output, String>
where
    S: AsRef<str> + std::fmt::Debug,
{
    let mut command = vec![
        "git".to_string(),
        "--literal-pathspecs".into(),
        "--no-pager".into(),
        "--no-optional-locks".into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-c".into(),
        "log.showSignature=false".into(),
        "-c".into(),
        "diff.ignoreSubmodules=all".into(),
        "-c".into(),
        "status.submoduleSummary=false".into(),
    ];
    // Even status/diff can run clean/process filters. Inspect configuration
    // without invoking them and fail closed rather than silently change Git's
    // normalization semantics. The user can explicitly approve a command within the sandbox policy.
    let mut config = command.clone();
    config.extend([
        "config".into(),
        "--null".into(),
        "--name-only".into(),
        "--get-regexp".into(),
        "^filter\\..*\\.(clean|smudge|process)$".into(),
    ]);
    let configured = crate::process::run(
        &config,
        workspace_root,
        std::time::Duration::from_secs(5),
        &crate::TaskControl::default(),
    )
    .map_err(|e| format!("Cannot inspect Git filter configuration: {e}"))?;
    if configured.timed_out
        || configured.truncated
        || !configured.stderr.is_empty()
        || configured.status.code().is_none()
        || !matches!(configured.status.code(), Some(0 | 1))
    {
        return Err("Cannot safely inspect Git filter configuration".into());
    }
    if !configured.stdout.is_empty() {
        return Err("Git content filters are configured; use an explicitly approved run_command to inspect this repository".into());
    }
    command.extend(args.iter().map(|s| s.as_ref().to_string()));
    let captured = crate::process::run(
        &command,
        workspace_root,
        std::time::Duration::from_secs(5),
        &crate::TaskControl::default(),
    )
    .map_err(|e| format!("Failed to run git command {:?}: {e}", args))?;
    if captured.timed_out || captured.truncated {
        return Err("Git output incomplete (timeout or capture limit); narrow the request".into());
    }
    let output = std::process::Output {
        status: captured.status,
        stdout: captured.stdout,
        stderr: captured.stderr,
    };
    if output.status.success() {
        return Ok(output);
    }

    Err(format!(
        "Git command {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn stdout_to_string(output: &std::process::Output) -> Result<String, String> {
    String::from_utf8(output.stdout.clone())
        .map_err(|e| format!("Command output was not valid UTF-8: {e}"))
}

fn git_relative_path(workspace_root: &Path, requested_path: &str) -> Result<String, ToolError> {
    let workspace_canonical = workspace_root.canonicalize().map_err(|e| {
        format!(
            "Failed to resolve workspace root '{}': {e}",
            workspace_root.display()
        )
    })?;
    let resolved = resolve_workspace_path(workspace_root, requested_path)?;
    let relative = resolved.strip_prefix(&workspace_canonical).map_err(|e| {
        format!(
            "Failed to derive git-relative path for '{}': {e}",
            requested_path
        )
    })?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

pub(crate) const DIRTY_WORKTREE_WARNING: &str = "Warning: git worktree has uncommitted changes";

pub(crate) fn maybe_prepend_dirty_worktree_warning(
    workspace_root: &Path,
    content: String,
) -> String {
    if git_worktree_is_dirty(workspace_root) {
        format!("{DIRTY_WORKTREE_WARNING}\n{content}")
    } else {
        content
    }
}

fn git_worktree_is_dirty(workspace_root: &Path) -> bool {
    run_git_capture(workspace_root, &["status", "--porcelain"])
        .is_ok_and(|output| !output.stdout.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::ContentBlock;
    use crate::test_support::{git_stdout, init_git_repo, make_temp_dir};
    use serde_json::json;
    use std::fs;

    #[test]
    fn git_branch_returns_current_branch() {
        let workspace = make_temp_dir("git-branch");
        init_git_repo(workspace.path());

        let output = git_branch_from_workspace(workspace.path(), &json!({})).unwrap();

        assert!(output.contains("Branch: main"));
    }

    #[test]
    fn git_status_reports_branch_and_dirty_entries() {
        let workspace = make_temp_dir("git-status");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();
        fs::write(workspace.path().join("untracked.txt"), "new\n").unwrap();

        let output = git_status_from_workspace(workspace.path(), &json!({})).unwrap();

        assert!(output.contains("Branch: main"));
        assert!(output.contains(" M tracked.txt"));
        assert!(output.contains("?? untracked.txt"));
    }

    #[test]
    fn git_diff_returns_patch_for_requested_path() {
        let workspace = make_temp_dir("git-diff");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();

        let output =
            git_diff_from_workspace(workspace.path(), &json!({ "path": "tracked.txt" })).unwrap();

        assert!(output.contains("Path: tracked.txt"));
        assert!(output.contains("--- a/tracked.txt"));
        assert!(output.contains("+++ b/tracked.txt"));
        assert!(output.contains("-initial"));
        assert!(output.contains("+changed"));
    }

    #[test]
    fn git_diff_leaves_out_files_that_may_hold_credentials() {
        let workspace = make_temp_dir("git-diff-secrets");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join(".env"), "TOKEN=old\n").unwrap();
        crate::test_support::run_git(workspace.path(), &["add", "-f", ".env"]);
        crate::test_support::run_git(workspace.path(), &["commit", "-m", "env"]);
        fs::write(workspace.path().join(".env"), "TOKEN=new-secret\n").unwrap();
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();

        let output = git_diff_from_workspace(workspace.path(), &json!({})).unwrap();
        assert!(output.contains("+changed"), "{output}");
        assert!(!output.contains("new-secret"), "{output}");
        assert!(output.contains(".env"), "{output}");

        // Named on its own, it was approved first, so it is shown.
        let named = git_diff_from_workspace(workspace.path(), &json!({"path": ".env"})).unwrap();
        assert!(named.contains("new-secret"), "{named}");
    }

    #[test]
    fn git_log_returns_recent_commits() {
        let workspace = make_temp_dir("git-log");
        init_git_repo(workspace.path());

        let output = git_log_from_workspace(workspace.path(), &json!({ "max_count": 1 })).unwrap();

        assert!(output.contains("Count: 1"));
        assert!(output.contains("initial commit"));
    }

    #[test]
    fn scoped_commit_preserves_unrelated_index_and_untracked_files() {
        let workspace = make_temp_dir("scoped-commit");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "agent change\n").unwrap();
        fs::write(workspace.path().join("staged.txt"), "human staged\n").unwrap();
        git_stdout(workspace.path(), &["add", "staged.txt"]);
        fs::write(workspace.path().join("untracked.txt"), "human new\n").unwrap();
        let control = crate::TaskControl::default();
        control.enable_interactive_approval();
        let result = crate::tools::run_tool_call_controlled(
            workspace.path(),
            "commit",
            "git_commit",
            &json!({"message":"agent change","paths":["tracked.txt"]}),
            &control,
            &mut |e| {
                if let AgentProgress::ApprovalRequested { request_id, .. } = e {
                    control.answer_approval(request_id, true);
                }
            },
        );
        assert!(
            matches!(result, ContentBlock::ToolResult { is_error: None, .. }),
            "{result:?}"
        );
        assert_eq!(
            git_stdout(workspace.path(), &["diff", "--cached", "--name-only"]).trim(),
            "staged.txt"
        );
        assert_eq!(
            git_stdout(
                workspace.path(),
                &["show", "--pretty=", "--name-only", "HEAD"]
            )
            .trim(),
            "tracked.txt"
        );
        assert!(workspace.path().join("untracked.txt").exists());
    }

    fn approved_commit(root: &Path, paths: &[&str]) -> ContentBlock {
        let control = crate::TaskControl::default();
        control.enable_interactive_approval();
        crate::tools::run_tool_call_controlled(
            root,
            "commit",
            "git_commit",
            &json!({"message": "scoped", "paths": paths}),
            &control,
            &mut |e| {
                if let AgentProgress::ApprovalRequested { request_id, .. } = e {
                    control.answer_approval(request_id, true);
                }
            },
        )
    }

    fn committed(root: &Path) -> String {
        git_stdout(root, &["show", "--pretty=", "--name-status", "HEAD"])
    }

    fn succeeded(result: &ContentBlock) -> bool {
        matches!(result, ContentBlock::ToolResult { is_error: None, .. })
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_path_commits_the_link_and_never_its_target() {
        use std::os::unix::fs::symlink;
        let workspace = make_temp_dir("commit-symlink");
        let root = workspace.path();
        init_git_repo(root);
        fs::write(root.join("target.txt"), "target v1\n").unwrap();
        fs::write(root.join("other.txt"), "other\n").unwrap();
        symlink("target.txt", root.join("link")).unwrap();
        git_stdout(root, &["add", "target.txt", "other.txt", "link"]);
        git_stdout(root, &["commit", "-m", "base"]);

        // The target changes; committing the link must not pick it up.
        fs::write(root.join("target.txt"), "target v2\n").unwrap();
        let review = commit_review(root, &json!({"paths": ["link"]}));
        assert!(
            review.is_err(),
            "the link itself did not change: {review:?}"
        );
        assert!(!succeeded(&approved_commit(root, &["link"])));
        assert_eq!(
            git_stdout(root, &["show", "HEAD:target.txt"]),
            "target v1\n"
        );

        // Retargeting the link commits only the link.
        fs::remove_file(root.join("link")).unwrap();
        symlink("other.txt", root.join("link")).unwrap();
        let review = commit_review(root, &json!({"paths": ["link"]})).unwrap();
        assert!(
            review.contains("+other.txt") && !review.contains("target v2"),
            "{review}"
        );
        let result = approved_commit(root, &["link"]);
        assert!(succeeded(&result), "{result:?}");
        assert_eq!(committed(root).trim(), "M\tlink");
        assert_eq!(git_stdout(root, &["show", "HEAD:link"]), "other.txt");
        assert_eq!(
            git_stdout(root, &["show", "HEAD:target.txt"]),
            "target v1\n"
        );

        // Deleting the link commits the deletion, not the target's.
        fs::remove_file(root.join("link")).unwrap();
        let result = approved_commit(root, &["link"]);
        assert!(succeeded(&result), "{result:?}");
        assert_eq!(committed(root).trim(), "D\tlink");
        assert!(git_stdout(root, &["status", "--porcelain"]).contains(" M target.txt"));

        // The target's change is committed by naming the target.
        let result = approved_commit(root, &["target.txt"]);
        assert!(succeeded(&result), "{result:?}");
        assert_eq!(committed(root).trim(), "M\ttarget.txt");
    }

    #[test]
    fn a_named_new_file_is_committed_and_a_failed_commit_leaves_it_untracked() {
        let workspace = make_temp_dir("commit-new");
        let root = workspace.path();
        init_git_repo(root);
        fs::write(root.join("new.rs"), "fn f() {}\n").unwrap();
        fs::write(root.join("unrelated.txt"), "human work\n").unwrap();
        let review = commit_review(root, &json!({"paths": ["new.rs"]})).unwrap();
        assert!(review.contains("New file: new.rs"), "{review}");
        assert!(review.contains("+fn f() {}"), "{review}");
        let requested = review.split("Requested diff:").nth(1).unwrap();
        assert!(!requested.contains("unrelated"), "{review}");

        let result = approved_commit(root, &["new.rs"]);
        assert!(succeeded(&result), "{result:?}");
        assert_eq!(committed(root).trim(), "A\tnew.rs");
        assert!(git_stdout(root, &["status", "--porcelain"]).contains("?? unrelated.txt"));

        // A hook that fails: the new file must not stay half-added.
        fs::write(root.join("second.rs"), "fn g() {}\n").unwrap();
        let hook = root.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(!succeeded(&approved_commit(root, &["second.rs"])));
        assert!(git_stdout(root, &["ls-files", "second.rs"]).is_empty());
        assert!(git_stdout(root, &["status", "--porcelain"]).contains("?? second.rs"));
    }

    #[test]
    fn the_global_identity_is_read_outside_any_repository() {
        let home = make_temp_dir("git-home");
        fs::write(
            home.path().join(".gitconfig"),
            "[user]\n\tname = Global Person\n\temail = global@example.com\n",
        )
        .unwrap();
        assert_eq!(
            host_git_config("user.name", home.path().as_os_str(), None).as_deref(),
            Some("Global Person")
        );
        let empty = make_temp_dir("git-home-empty");
        assert_eq!(
            host_git_config("user.name", empty.path().as_os_str(), None),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn commit_paths_keep_the_workspace_and_state_boundaries() {
        use std::os::unix::fs::symlink;
        let workspace = make_temp_dir("commit-boundary");
        let outside = make_temp_dir("commit-outside");
        let root = workspace.path();
        init_git_repo(root);
        symlink(outside.path(), root.join("away")).unwrap();
        symlink(".git", root.join("gitdir")).unwrap();
        fs::create_dir(root.join("dir")).unwrap();
        let canonical = root.canonicalize().unwrap();

        for (path, message) in [
            ("away/file", "outside"),
            ("../escape", "outside"),
            ("gitdir/config", "git state"),
            (".git/config", "git state"),
            (".socium/edit_history.json", "git state"),
            ("dir", "files only"),
            ("dir/..", "files only"),
            ("tracked.txt/x", "not a directory"),
        ] {
            let err = commit_pathspec(&canonical, path).unwrap_err().to_string();
            assert!(err.to_lowercase().contains(message), "{path}: {err}");
        }
        // A link that leaves the workspace is committed as a link; Git
        // stores where it points and never reads the target.
        symlink(outside.path().join("secret"), root.join("outlink")).unwrap();
        assert_eq!(commit_pathspec(&canonical, "outlink").unwrap(), "outlink");
        assert_eq!(
            commit_pathspec(&canonical, "./sub/../tracked.txt").unwrap(),
            "tracked.txt"
        );
    }
}
