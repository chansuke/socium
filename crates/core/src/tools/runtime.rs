use crate::agent::types::ContentBlock;
use crate::constants::*;
use serde_json::{Value, json};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) fn run_tool_calls(workspace_root: &Path, blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => {
                Some(run_tool_call(workspace_root, id, name, input))
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn run_tool_call(
    workspace_root: &Path,
    tool_use_id: &str,
    name: &str,
    input: &Value,
) -> ContentBlock {
    let result = match name {
        "read_file" => read_file_from_workspace(workspace_root, input),
        "list_files" => list_files_from_workspace(workspace_root, input),
        "search_text" => search_text_from_workspace(workspace_root, input),
        "apply_patch" => apply_patch_from_workspace(workspace_root, input),
        "git_branch" => git_branch_from_workspace(workspace_root, input),
        "git_status" => git_status_from_workspace(workspace_root, input),
        "git_diff" => git_diff_from_workspace(workspace_root, input),
        "git_log" => git_log_from_workspace(workspace_root, input),
        "find_symbol" => find_symbol_in_workspace(workspace_root, input),
        "run_command" => run_command_from_workspace(workspace_root, input),
        TEXT_EDITOR_TOOL_NAME => text_editor_from_workspace(workspace_root, input),
        other => Err(format!("Unsupported tool: {other}")),
    };

    match result {
        Ok(content) => ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content,
            is_error: None,
        },
        Err(message) => ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: message,
            is_error: Some(true),
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum CommandApproval {
    None,
    WorkspaceWrite,
    Destructive,
}

impl CommandApproval {
    fn parse(input: Option<&str>) -> Result<Self, String> {
        match input {
            None => Ok(Self::None),
            Some("workspace-write") => Ok(Self::WorkspaceWrite),
            Some("destructive") => Ok(Self::Destructive),
            Some(other) => Err(format!(
                "run_command approval must be either 'workspace-write' or 'destructive', got '{other}'"
            )),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::WorkspaceWrite => "workspace-write",
            Self::Destructive => "destructive",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum CommandPermission {
    SafeReadOnly,
    WorkspaceWrite,
    Destructive,
}

impl CommandPermission {
    fn label(self) -> &'static str {
        match self {
            Self::SafeReadOnly => "safe-read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::Destructive => "destructive",
        }
    }

    fn required_approval(self) -> CommandApproval {
        match self {
            Self::SafeReadOnly => CommandApproval::None,
            Self::WorkspaceWrite => CommandApproval::WorkspaceWrite,
            Self::Destructive => CommandApproval::Destructive,
        }
    }
}

struct CommandExecution {
    permission: CommandPermission,
    command_display: String,
    cwd_display: String,
    timeout_ms: u64,
    timed_out: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

struct SearchMatch {
    path: String,
    line_number: usize,
    text: String,
}

enum PatchAction {
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<PatchHunk>,
    },
}

struct PatchHunk {
    old_lines: Vec<String>,
    new_lines: Vec<String>,
}

pub(crate) fn run_command_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    let input_object = input
        .as_object()
        .ok_or("run_command requires an object input")?;
    let command = require_string_array_field(input_object, "command", "run_command")?;
    let cwd = optional_string_field(input_object, "cwd", "run_command cwd")?.unwrap_or(".");
    let timeout_ms = optional_timeout_ms(input_object)?;
    let approval = CommandApproval::parse(optional_string_field(
        input_object,
        "approval",
        "run_command approval",
    )?)?;
    let permission = classify_command(&command);
    let required_approval = permission.required_approval();

    if approval < required_approval {
        return Err(format!(
            "Command '{}' is classified as {} and requires {} approval. Rerun with approval set to '{}'.",
            display_command(&command),
            permission.label(),
            required_approval.label(),
            required_approval.label()
        ));
    }

    let resolved_cwd = resolve_workspace_command_cwd(workspace_root, cwd)?;
    let execution = execute_command(&command, &resolved_cwd, cwd, timeout_ms, permission)?;

    let output = format_command_execution(&execution);
    if execution.timed_out || execution.exit_code.unwrap_or_default() != 0 {
        Err(output)
    } else {
        Ok(output)
    }
}

fn execute_command(
    command: &[String],
    cwd: &Path,
    cwd_display: &str,
    timeout_ms: u64,
    permission: CommandPermission,
) -> Result<CommandExecution, String> {
    let program = command
        .first()
        .ok_or("run_command command must contain at least one element")?;
    let args = &command[1..];

    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "Failed to spawn command '{}': {e}",
                display_command(command)
            )
        })?;

    let timeout = Duration::from_millis(timeout_ms);
    let start = Instant::now();
    let mut timed_out = false;

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    child.kill().map_err(|e| {
                        format!(
                            "Failed to terminate timed out command '{}': {e}",
                            display_command(command)
                        )
                    })?;
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                return Err(format!(
                    "Failed while waiting for command '{}': {e}",
                    display_command(command)
                ));
            }
        }
    }

    let output = child.wait_with_output().map_err(|e| {
        format!(
            "Failed to collect output for command '{}': {e}",
            display_command(command)
        )
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    Ok(CommandExecution {
        permission,
        command_display: display_command(command),
        cwd_display: cwd_display.to_string(),
        timeout_ms,
        timed_out,
        exit_code: output.status.code(),
        stdout,
        stderr,
    })
}

fn format_command_execution(execution: &CommandExecution) -> String {
    let stdout = format_command_stream(&execution.stdout);
    let stderr = format_command_stream(&execution.stderr);
    let exit_code = match execution.exit_code {
        Some(code) => code.to_string(),
        None => "terminated".to_string(),
    };

    format!(
        "Command: {}\nCwd: {}\nPermission: {}\nTimeout: {} ms\nTimed out: {}\nExit code: {}\nStdout:\n{}\nStderr:\n{}",
        execution.command_display,
        execution.cwd_display,
        execution.permission.label(),
        execution.timeout_ms,
        execution.timed_out,
        exit_code,
        stdout,
        stderr
    )
}

fn format_command_stream(stream: &str) -> String {
    if stream.is_empty() {
        return "(empty)".to_string();
    }

    let (truncated, was_truncated) = truncate_by_characters(stream, MAX_COMMAND_OUTPUT_CHARACTERS);
    if was_truncated {
        format!(
            "{truncated}\n(truncated: showing first {} characters)",
            MAX_COMMAND_OUTPUT_CHARACTERS
        )
    } else {
        truncated
    }
}

pub(crate) fn search_text_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    let input_object = input
        .as_object()
        .ok_or("search_text requires an object input")?;
    let pattern = require_string_field(input_object, "pattern", "search_text")?;
    let path = optional_string_field(input_object, "path", "search_text path")?.unwrap_or(".");
    let glob = optional_string_field(input_object, "glob", "search_text glob")?;
    let max_results = optional_bounded_usize_field(
        input_object,
        "max_results",
        "search_text",
        DEFAULT_SEARCH_RESULTS,
        MAX_SEARCH_RESULTS,
    )?;
    let resolved_path = resolve_workspace_path(workspace_root, path)?;

    let mut args = vec![
        "--line-number".to_string(),
        "--with-filename".to_string(),
        "--color".to_string(),
        "never".to_string(),
        "--no-heading".to_string(),
        "--smart-case".to_string(),
    ];
    if let Some(glob) = glob {
        args.push("--glob".to_string());
        args.push(glob.to_string());
    }
    args.push(pattern.to_string());
    args.push(resolved_path.to_string_lossy().into_owned());

    let output = Command::new("rg")
        .args(&args)
        .current_dir(workspace_root)
        .output()
        .map_err(|e| format!("Failed to run ripgrep for search_text: {e}"))?;
    let matches = parse_rg_matches(workspace_root, &output.stdout)?;
    let formatted = format_search_results("Pattern", pattern, path, glob, max_results, matches);

    match output.status.code() {
        Some(0) | Some(1) => Ok(formatted),
        Some(code) => Err(format!(
            "search_text failed with exit code {code}\n{formatted}"
        )),
        None => Err(format!("search_text terminated unexpectedly\n{formatted}")),
    }
}

pub(crate) fn find_symbol_in_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    let input_object = input
        .as_object()
        .ok_or("find_symbol requires an object input")?;
    let symbol = require_string_field(input_object, "symbol", "find_symbol")?;
    let path = optional_string_field(input_object, "path", "find_symbol path")?.unwrap_or(".");
    let max_results = optional_bounded_usize_field(
        input_object,
        "max_results",
        "find_symbol",
        DEFAULT_SEARCH_RESULTS,
        MAX_SEARCH_RESULTS,
    )?;
    let resolved_path = resolve_workspace_path(workspace_root, path)?;

    let output = Command::new("rg")
        .args([
            "--line-number",
            "--with-filename",
            "--color",
            "never",
            "--no-heading",
            "--fixed-strings",
            "--glob",
            "*.rs",
            symbol,
            resolved_path.to_string_lossy().as_ref(),
        ])
        .current_dir(workspace_root)
        .output()
        .map_err(|e| format!("Failed to run ripgrep for find_symbol: {e}"))?;
    let matches = parse_rg_matches(workspace_root, &output.stdout)?
        .into_iter()
        .filter(|entry| looks_like_rust_symbol_declaration(&entry.text, symbol))
        .collect::<Vec<_>>();
    let formatted =
        format_search_results("Symbol", symbol, path, Some("*.rs"), max_results, matches);

    match output.status.code() {
        Some(0) | Some(1) => Ok(formatted),
        Some(code) => Err(format!(
            "find_symbol failed with exit code {code}\n{formatted}"
        )),
        None => Err(format!("find_symbol terminated unexpectedly\n{formatted}")),
    }
}

pub(crate) fn apply_patch_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    let input_object = input
        .as_object()
        .ok_or("apply_patch requires an object input")?;
    let patch = require_string_field(input_object, "patch", "apply_patch")?;
    let dry_run =
        optional_bool_field(input_object, "dry_run", "apply_patch dry_run")?.unwrap_or(false);
    let actions = parse_patch_actions(patch)?;
    if dry_run {
        return preview_patch_actions(workspace_root, &actions);
    }
    let mut results = Vec::new();

    for action in actions {
        match action {
            PatchAction::Add { path, content } => {
                let target_path = resolve_workspace_path_for_create(workspace_root, &path)?;
                if target_path.exists() {
                    return Err(format!("Cannot add file '{}': path already exists", path));
                }
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        format!("Failed to create parent directories for '{}': {e}", path)
                    })?;
                }
                fs::write(&target_path, content)
                    .map_err(|e| format!("Failed to write new file '{}': {e}", path))?;
                results.push(format!("Added file: {path}"));
            }
            PatchAction::Delete { path } => {
                let target_path = resolve_workspace_path(workspace_root, &path)?;
                let metadata = fs::metadata(&target_path)
                    .map_err(|e| format!("Failed to inspect file '{}': {e}", path))?;
                if !metadata.is_file() {
                    return Err(format!("Path is not a file: '{}'", path));
                }
                fs::remove_file(&target_path)
                    .map_err(|e| format!("Failed to delete file '{}': {e}", path))?;
                results.push(format!("Deleted file: {path}"));
            }
            PatchAction::Update {
                path,
                move_to,
                hunks,
            } => {
                let source_path = resolve_workspace_path(workspace_root, &path)?;
                let metadata = fs::metadata(&source_path)
                    .map_err(|e| format!("Failed to inspect file '{}': {e}", path))?;
                if !metadata.is_file() {
                    return Err(format!("Path is not a file: '{}'", path));
                }
                let content = read_utf8_file_with_limit(&source_path, &path, MAX_READ_FILE_BYTES)?;
                let updated = apply_patch_hunks_to_text(&content, &path, &hunks)?;
                let target_display = move_to.clone().unwrap_or_else(|| path.clone());
                let target_path = if let Some(move_to) = &move_to {
                    let target_path = resolve_workspace_path_for_create(workspace_root, move_to)?;
                    if move_to != &path && target_path.exists() {
                        return Err(format!(
                            "Cannot move file '{}' to '{}': destination already exists",
                            path, move_to
                        ));
                    }
                    target_path
                } else {
                    source_path.clone()
                };
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        format!(
                            "Failed to create parent directories for '{}': {e}",
                            target_display
                        )
                    })?;
                }
                fs::write(&target_path, updated).map_err(|e| {
                    format!("Failed to write updated file '{}': {e}", target_display)
                })?;
                if let Some(move_to) = move_to {
                    if move_to != path {
                        fs::remove_file(&source_path).map_err(|e| {
                            format!("Failed to remove moved source file '{}': {e}", path)
                        })?;
                        results.push(format!("Moved file: {path} -> {move_to}"));
                    }
                    results.push(format!("Updated file: {move_to}"));
                } else {
                    results.push(format!("Updated file: {path}"));
                }
            }
        }
    }

    Ok(results.join("\n"))
}

fn parse_patch_actions(patch: &str) -> Result<Vec<PatchAction>, String> {
    let lines = patch.lines().collect::<Vec<_>>();
    if lines.first().copied() != Some("*** Begin Patch") {
        return Err("apply_patch patch must start with '*** Begin Patch'".to_string());
    }
    if lines.last().copied() != Some("*** End Patch") {
        return Err("apply_patch patch must end with '*** End Patch'".to_string());
    }

    let mut actions = Vec::new();
    let mut index = 1usize;
    while index < lines.len() - 1 {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut content_lines = Vec::new();
            while index < lines.len() - 1 && !lines[index].starts_with("*** ") {
                let patch_line = lines[index];
                let added = patch_line
                    .strip_prefix('+')
                    .ok_or_else(|| format!("Add File section for '{}' requires '+' lines", path))?;
                content_lines.push(added.to_string());
                index += 1;
            }
            actions.push(PatchAction::Add {
                path: path.to_string(),
                content: content_lines.join("\n"),
            });
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            actions.push(PatchAction::Delete {
                path: path.to_string(),
            });
            index += 1;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let move_to = if index < lines.len() - 1 {
                lines[index]
                    .strip_prefix("*** Move to: ")
                    .map(ToString::to_string)
            } else {
                None
            };
            if move_to.is_some() {
                index += 1;
            }
            let mut body = Vec::new();
            while index < lines.len() - 1 && !lines[index].starts_with("*** ") {
                body.push(lines[index]);
                index += 1;
            }
            let hunks = parse_patch_hunks(path, &body)?;
            actions.push(PatchAction::Update {
                path: path.to_string(),
                move_to,
                hunks,
            });
            continue;
        }
        return Err(format!("Unsupported patch header: {line}"));
    }

    if actions.is_empty() {
        return Err("apply_patch patch did not contain any file actions".to_string());
    }

    Ok(actions)
}

fn parse_patch_hunks(path: &str, lines: &[&str]) -> Result<Vec<PatchHunk>, String> {
    let mut hunks = Vec::new();
    let mut old_lines = Vec::new();
    let mut new_lines = Vec::new();

    for line in lines {
        if line.starts_with("@@") {
            if !old_lines.is_empty() || !new_lines.is_empty() {
                hunks.push(PatchHunk {
                    old_lines,
                    new_lines,
                });
                old_lines = Vec::new();
                new_lines = Vec::new();
            }
            continue;
        }
        let (prefix, content) = line
            .chars()
            .next()
            .map(|prefix| (prefix, &line[prefix.len_utf8()..]))
            .ok_or_else(|| format!("Invalid empty patch line in '{}'", path))?;
        match prefix {
            ' ' => {
                old_lines.push(content.to_string());
                new_lines.push(content.to_string());
            }
            '-' => old_lines.push(content.to_string()),
            '+' => new_lines.push(content.to_string()),
            _ => {
                return Err(format!(
                    "Invalid patch line prefix '{}' in update for '{}'",
                    prefix, path
                ));
            }
        }
    }

    if !old_lines.is_empty() || !new_lines.is_empty() {
        hunks.push(PatchHunk {
            old_lines,
            new_lines,
        });
    }
    if hunks.is_empty() {
        return Err(format!(
            "Update File section for '{}' did not contain any hunks",
            path
        ));
    }

    Ok(hunks)
}

fn apply_patch_hunks_to_text(
    content: &str,
    path: &str,
    hunks: &[PatchHunk],
) -> Result<String, String> {
    let trailing_newline = content.ends_with('\n');
    let stripped = content.strip_suffix('\n').unwrap_or(content);
    let mut lines = if stripped.is_empty() {
        Vec::new()
    } else {
        stripped
            .split('\n')
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    };
    let mut search_start = 0usize;

    for hunk in hunks {
        if hunk.old_lines.is_empty() {
            return Err(format!(
                "apply_patch does not support insertion-only hunks yet for '{}'",
                path
            ));
        }
        let position = find_hunk_position(&lines, &hunk.old_lines, search_start)
            .ok_or_else(|| format!("Failed to match patch hunk in '{}'", path))?;
        lines.splice(
            position..position + hunk.old_lines.len(),
            hunk.new_lines.clone(),
        );
        search_start = position;
    }

    let mut updated = lines.join("\n");
    if trailing_newline && !updated.is_empty() {
        updated.push('\n');
    }
    Ok(updated)
}

fn find_hunk_position(lines: &[String], needle: &[String], search_start: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > lines.len() {
        return None;
    }

    (search_start..=lines.len().saturating_sub(needle.len()))
        .find(|start| lines[*start..*start + needle.len()] == *needle)
}

fn preview_patch_actions(workspace_root: &Path, actions: &[PatchAction]) -> Result<String, String> {
    let mut lines = vec!["Preview: true".to_string()];
    for action in actions {
        match action {
            PatchAction::Add { path, content } => {
                lines.push(format!("*** Add File: {path}"));
                lines.push(format!("+++ {path}"));
                lines.extend(content.lines().map(|line| format!("+{line}")));
            }
            PatchAction::Delete { path } => {
                let target_path = resolve_workspace_path(workspace_root, path)?;
                let content = read_utf8_file_with_limit(&target_path, path, MAX_READ_FILE_BYTES)?;
                lines.push(format!("*** Delete File: {path}"));
                lines.push(format!("--- {path}"));
                lines.extend(content.lines().map(|line| format!("-{line}")));
            }
            PatchAction::Update {
                path,
                move_to,
                hunks,
            } => {
                let target = move_to.as_deref().unwrap_or(path);
                lines.push(format!("*** Update File: {path}"));
                if let Some(move_to) = move_to {
                    lines.push(format!("*** Move to: {move_to}"));
                }
                lines.push(format!("--- {path}"));
                lines.push(format!("+++ {target}"));
                for hunk in hunks {
                    lines.push("@@".to_string());
                    for old_line in &hunk.old_lines {
                        if !hunk.new_lines.contains(old_line) {
                            lines.push(format!("-{old_line}"));
                        }
                    }
                    for new_line in &hunk.new_lines {
                        if !hunk.old_lines.contains(new_line) {
                            lines.push(format!("+{new_line}"));
                        }
                    }
                }
            }
        }
    }
    Ok(lines.join("\n"))
}

pub(crate) fn git_branch_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    require_object_input(input, "git_branch")?;
    let output = run_git_capture(workspace_root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = stdout_to_string(&output)?.trim().to_string();
    Ok(format!("Branch: {branch}"))
}

pub(crate) fn git_status_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    let input_object = require_object_input(input, "git_status")?;
    let path = optional_string_field(input_object, "path", "git_status path")?;
    let mut args = vec![
        "status".to_string(),
        "--short".to_string(),
        "--branch".to_string(),
    ];
    if let Some(path) = path {
        args.push("--".to_string());
        args.push(git_relative_path(workspace_root, path)?);
    }
    let output = run_git_capture_owned(workspace_root, &args)?;
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

pub(crate) fn git_diff_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    let input_object = require_object_input(input, "git_diff")?;
    let path = optional_string_field(input_object, "path", "git_diff path")?;
    let mut args = vec!["diff".to_string()];
    if let Some(path) = path {
        args.push("--".to_string());
        args.push(git_relative_path(workspace_root, path)?);
    }
    let output = run_git_capture_owned(workspace_root, &args)?;
    let stdout = stdout_to_string(&output)?;
    let mut result = Vec::new();
    if let Some(path) = path {
        result.push(format!("Path: {path}"));
    }
    if stdout.trim().is_empty() {
        result.push("(no diff)".to_string());
    } else {
        result.push(stdout);
    }
    Ok(result.join("\n"))
}

pub(crate) fn git_log_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    let input_object = require_object_input(input, "git_log")?;
    let max_count = optional_bounded_usize_field(input_object, "max_count", "git_log", 10, 100)?;
    let output = run_git_capture_owned(
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

fn require_object_input<'a>(
    input: &'a Value,
    tool_name: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    input
        .as_object()
        .ok_or_else(|| format!("{tool_name} requires an object input"))
}

fn run_git_capture(workspace_root: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let owned = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_git_capture_owned(workspace_root, &owned)
}

fn run_git_capture_owned(
    workspace_root: &Path,
    args: &[String],
) -> Result<std::process::Output, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace_root)
        .output()
        .map_err(|e| format!("Failed to run git command {:?}: {e}", args))?;
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

fn git_relative_path(workspace_root: &Path, requested_path: &str) -> Result<String, String> {
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

fn parse_rg_matches(workspace_root: &Path, stdout: &[u8]) -> Result<Vec<SearchMatch>, String> {
    let text = String::from_utf8(stdout.to_vec())
        .map_err(|e| format!("ripgrep returned non UTF-8 output: {e}"))?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| parse_rg_match_line(workspace_root, line))
        .collect()
}

fn parse_rg_match_line(workspace_root: &Path, line: &str) -> Result<SearchMatch, String> {
    let mut parts = line.splitn(3, ':');
    let raw_path = parts
        .next()
        .ok_or_else(|| format!("Invalid ripgrep output line: '{line}'"))?;
    let raw_line_number = parts
        .next()
        .ok_or_else(|| format!("Invalid ripgrep output line: '{line}'"))?;
    let text = parts
        .next()
        .ok_or_else(|| format!("Invalid ripgrep output line: '{line}'"))?;
    let line_number = raw_line_number
        .parse::<usize>()
        .map_err(|e| format!("Invalid ripgrep line number '{raw_line_number}': {e}"))?;
    let path = normalize_match_path(workspace_root, raw_path);

    Ok(SearchMatch {
        path,
        line_number,
        text: text.to_string(),
    })
}

fn normalize_match_path(workspace_root: &Path, raw_path: &str) -> String {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        path.strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    } else {
        raw_path.replace('\\', "/")
    }
}

fn format_search_results(
    label: &str,
    query: &str,
    path: &str,
    glob: Option<&str>,
    max_results: usize,
    matches: Vec<SearchMatch>,
) -> String {
    let total = matches.len();
    let truncated = total > max_results;
    let mut lines = Vec::new();
    lines.push(format!("{label}: {query}"));
    lines.push(format!("Path: {path}"));
    if let Some(glob) = glob {
        lines.push(format!("Glob: {glob}"));
    }

    if total == 0 {
        lines.push("(no matches)".to_string());
        return lines.join("\n");
    }

    lines.extend(
        matches
            .into_iter()
            .take(max_results)
            .map(|entry| format!("{}:{}:{}", entry.path, entry.line_number, entry.text)),
    );
    if truncated {
        lines.push(format!(
            "(truncated: showing first {max_results} matches; more matches omitted)"
        ));
    }
    lines.join("\n")
}

fn looks_like_rust_symbol_declaration(line: &str, symbol: &str) -> bool {
    let trimmed = line.trim_start();
    let declaration_keywords = [
        "fn", "struct", "enum", "trait", "type", "mod", "const", "static",
    ];
    if trimmed.starts_with("impl ") {
        return contains_symbol_token(trimmed, symbol);
    }

    declaration_keywords.iter().any(|keyword| {
        let token = format!("{keyword} {symbol}");
        trimmed.contains(&token) && contains_symbol_token(trimmed, symbol)
    })
}

fn contains_symbol_token(line: &str, symbol: &str) -> bool {
    let bytes = line.as_bytes();
    let symbol_bytes = symbol.as_bytes();
    if symbol_bytes.is_empty() || bytes.len() < symbol_bytes.len() {
        return false;
    }

    for start in 0..=bytes.len() - symbol_bytes.len() {
        if &bytes[start..start + symbol_bytes.len()] != symbol_bytes {
            continue;
        }
        let before_ok = start == 0 || !is_symbol_char(bytes[start - 1] as char);
        let end = start + symbol_bytes.len();
        let after_ok = end == bytes.len() || !is_symbol_char(bytes[end] as char);
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

fn is_symbol_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn classify_command(command: &[String]) -> CommandPermission {
    let Some(program) = command.first() else {
        return CommandPermission::Destructive;
    };
    let program_name = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program.as_str());
    let first_arg = command.get(1).map(String::as_str).unwrap_or_default();

    if is_destructive_command(program_name, first_arg) {
        CommandPermission::Destructive
    } else if is_safe_read_only_command(program_name, first_arg) {
        CommandPermission::SafeReadOnly
    } else {
        CommandPermission::WorkspaceWrite
    }
}

fn is_safe_read_only_command(program: &str, first_arg: &str) -> bool {
    matches!(
        program,
        "cat"
            | "echo"
            | "env"
            | "find"
            | "git"
            | "grep"
            | "head"
            | "ls"
            | "pwd"
            | "rg"
            | "sed"
            | "sort"
            | "stat"
            | "tail"
            | "uname"
            | "wc"
            | "which"
    ) && (program != "git"
        || matches!(
            first_arg,
            "branch" | "diff" | "log" | "rev-parse" | "show" | "status"
        ))
}

fn is_destructive_command(program: &str, first_arg: &str) -> bool {
    matches!(
        program,
        "chmod" | "chown" | "dd" | "fdisk" | "mkfs" | "mv" | "rm" | "rmdir"
    ) || (program == "git"
        && matches!(
            first_arg,
            "checkout"
                | "cherry-pick"
                | "clean"
                | "merge"
                | "rebase"
                | "reset"
                | "restore"
                | "revert"
                | "switch"
        ))
}

fn display_command(command: &[String]) -> String {
    command
        .iter()
        .map(|part| {
            if part.is_empty() || part.chars().any(char::is_whitespace) {
                format!("{part:?}")
            } else {
                part.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn text_editor_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    let input_object = input
        .as_object()
        .ok_or("text_editor tool requires an object input")?;
    let command = require_string_field(input_object, "command", "text_editor command")?;

    match command {
        "view" => text_editor_view(workspace_root, input_object),
        "str_replace" => text_editor_str_replace(workspace_root, input_object),
        "create" => text_editor_create(workspace_root, input_object),
        "insert" => text_editor_insert(workspace_root, input_object),
        "undo_edit" => {
            Err("undo_edit is not supported by this text editor implementation".to_string())
        }
        other => Err(format!("Unsupported text editor command: {other}")),
    }
}

pub(crate) fn text_editor_view(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, String> {
    let path = require_string_field(input, "path", "text_editor view path")?;
    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect path '{}': {e}", path))?;

    if metadata.is_dir() {
        return list_files_from_workspace(workspace_root, &json!({ "path": path }));
    }
    if !metadata.is_file() {
        return Err(format!("Path is neither a file nor directory: '{}'", path));
    }

    let content = read_utf8_file_with_limit(&resolved_path, path, MAX_TEXT_EDITOR_VIEW_READ_BYTES)?;
    let selected_content = apply_view_range(&content, input.get("view_range"))?;
    let (display_content, truncated) =
        truncate_by_characters(&selected_content, MAX_TEXT_EDITOR_VIEW_CHARACTERS);

    if truncated {
        Ok(format!(
            "Path: {}\n{}\n(truncated: showing first {} characters)",
            path, display_content, MAX_TEXT_EDITOR_VIEW_CHARACTERS
        ))
    } else {
        Ok(format!("Path: {}\n{}", path, display_content))
    }
}

pub(crate) fn text_editor_str_replace(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, String> {
    let path = require_string_field(input, "path", "text_editor str_replace path")?;
    let old_str = require_string_field(input, "old_str", "text_editor str_replace old_str")?;
    let new_str = require_string_field(input, "new_str", "text_editor str_replace new_str")?;
    if old_str.is_empty() {
        return Err("text_editor str_replace old_str cannot be empty".to_string());
    }

    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", path))?;
    if !metadata.is_file() {
        return Err(format!("Path is not a file: '{}'", path));
    }

    let content = read_utf8_file_with_limit(&resolved_path, path, MAX_READ_FILE_BYTES)?;
    let matches = content.match_indices(old_str).count();
    if matches == 0 {
        return Err(format!(
            "No matches found for old_str in file '{}'; replacement not applied",
            path
        ));
    }
    if matches > 1 {
        return Err(format!(
            "Multiple matches found for old_str in file '{}'; provide a more specific old_str",
            path
        ));
    }

    let updated = content.replacen(old_str, new_str, 1);
    fs::write(&resolved_path, updated)
        .map_err(|e| format!("Failed to write file '{}': {e}", path))?;
    Ok(format!("Path: {}\nReplaced 1 occurrence", path))
}

pub(crate) fn text_editor_create(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, String> {
    let path = require_string_field(input, "path", "text_editor create path")?;
    let file_text = require_string_field(input, "file_text", "text_editor create file_text")?;
    let target_path = resolve_workspace_path_for_create(workspace_root, path)?;

    if target_path.exists() {
        return Err(format!(
            "Cannot create file '{}': path already exists",
            path
        ));
    }
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directories for '{}': {e}", path))?;
    }
    fs::write(&target_path, file_text)
        .map_err(|e| format!("Failed to create file '{}': {e}", path))?;
    Ok(format!("Path: {}\nCreated file", path))
}

pub(crate) fn text_editor_insert(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, String> {
    let path = require_string_field(input, "path", "text_editor insert path")?;
    let insert_line = require_i64_field(input, "insert_line", "text_editor insert insert_line")?;
    if insert_line < 0 {
        return Err("text_editor insert insert_line must be >= 0".to_string());
    }

    let insert_text = require_string_field(input, "insert_text", "text_editor insert insert_text")?;

    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", path))?;
    if !metadata.is_file() {
        return Err(format!("Path is not a file: '{}'", path));
    }

    let content = read_utf8_file_with_limit(&resolved_path, path, MAX_READ_FILE_BYTES)?;
    let line_count = count_lines(&content);
    let line = insert_line as usize;
    if line > line_count {
        return Err(format!(
            "insert_line {} is out of range for '{}' (file has {} lines)",
            line, path, line_count
        ));
    }

    let insert_at = byte_index_after_line(&content, line).ok_or_else(|| {
        format!(
            "Failed to compute insertion point for line {} in '{}'",
            line, path
        )
    })?;
    let mut updated = String::with_capacity(content.len() + insert_text.len());
    updated.push_str(&content[..insert_at]);
    updated.push_str(insert_text);
    updated.push_str(&content[insert_at..]);

    fs::write(&resolved_path, updated)
        .map_err(|e| format!("Failed to write file '{}': {e}", path))?;
    Ok(format!("Path: {}\nInserted text after line {}", path, line))
}

pub(crate) fn require_string_field<'a>(
    input: &'a serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<&'a str, String> {
    input
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context} requires a string field: {field}"))
}

pub(crate) fn optional_string_field<'a>(
    input: &'a serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<Option<&'a str>, String> {
    match input.get(field) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("{context} requires a string field: {field}")),
    }
}

fn optional_bool_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<Option<bool>, String> {
    match input.get(field) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{context} requires a boolean field: {field}")),
    }
}

fn optional_bounded_usize_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
    default: usize,
    max: usize,
) -> Result<usize, String> {
    let Some(value) = input.get(field) else {
        return Ok(default);
    };
    let raw = value
        .as_u64()
        .ok_or_else(|| format!("{context} requires an integer field: {field}"))?;
    let parsed = usize::try_from(raw)
        .map_err(|_| format!("{context} field '{field}' is too large for this platform"))?;
    if !(1..=max).contains(&parsed) {
        return Err(format!(
            "{context} field '{field}' must be between 1 and {max}"
        ));
    }
    Ok(parsed)
}

pub(crate) fn require_string_array_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<Vec<String>, String> {
    let values = input
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{context} requires an array field: {field}"))?;
    if values.is_empty() {
        return Err(format!(
            "{context} requires a non-empty array field: {field}"
        ));
    }

    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| format!("{context} requires string array items in field: {field}"))
        })
        .collect()
}

pub(crate) fn require_i64_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<i64, String> {
    input
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{context} requires an integer field: {field}"))
}

fn optional_timeout_ms(input: &serde_json::Map<String, Value>) -> Result<u64, String> {
    let Some(timeout_ms) = input.get("timeout_ms") else {
        return Ok(DEFAULT_COMMAND_TIMEOUT_MS);
    };
    let timeout_ms = timeout_ms
        .as_u64()
        .ok_or("run_command timeout_ms requires an integer field: timeout_ms")?;
    if !(1..=MAX_COMMAND_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(format!(
            "run_command timeout_ms must be between 1 and {MAX_COMMAND_TIMEOUT_MS}"
        ));
    }
    Ok(timeout_ms)
}

pub(crate) fn apply_view_range(
    content: &str,
    view_range: Option<&Value>,
) -> Result<String, String> {
    let Some(view_range) = view_range else {
        return Ok(content.to_string());
    };
    let range = view_range
        .as_array()
        .ok_or("text_editor view_range must be an array: [start_line, end_line]")?;
    if range.len() != 2 {
        return Err("text_editor view_range must contain exactly two integers".to_string());
    }
    let start = range[0]
        .as_i64()
        .ok_or("text_editor view_range start must be an integer")?;
    let end = range[1]
        .as_i64()
        .ok_or("text_editor view_range end must be an integer")?;
    if start < 1 {
        return Err("text_editor view_range start must be >= 1".to_string());
    }
    if end < -1 || end == 0 {
        return Err("text_editor view_range end must be -1 or a positive integer".to_string());
    }
    if end != -1 && end < start {
        return Err("text_editor view_range end must be -1 or >= start".to_string());
    }

    let line_count = count_lines(content) as i64;
    if line_count == 0 {
        if start == 1 {
            return Ok(String::new());
        }
        return Err("text_editor view_range start is out of range for an empty file".to_string());
    }
    if start > line_count {
        return Err(format!(
            "text_editor view_range start {} is out of range (file has {} lines)",
            start, line_count
        ));
    }
    let last = if end == -1 {
        line_count
    } else {
        std::cmp::min(end, line_count)
    };
    let start_line = start as usize;
    let end_line = last as usize;

    let start_offset = line_start_offset(content, start_line).ok_or_else(|| {
        format!(
            "text_editor view_range start {} is out of range (file has {} lines)",
            start, line_count
        )
    })?;
    let end_offset = if end_line as i64 == line_count {
        content.len()
    } else {
        line_start_offset(content, end_line + 1).ok_or_else(|| {
            format!(
                "text_editor view_range end {} is out of range (file has {} lines)",
                end_line, line_count
            )
        })?
    };

    Ok(content[start_offset..end_offset].to_string())
}

pub(crate) fn truncate_by_characters(content: &str, max_characters: usize) -> (String, bool) {
    if max_characters == 0 {
        return (String::new(), !content.is_empty());
    }

    let mut end = 0usize;
    for (count, (index, ch)) in content.char_indices().enumerate() {
        if count == max_characters {
            return (content[..end].to_string(), true);
        }
        end = index + ch.len_utf8();
    }
    (content.to_string(), false)
}

pub(crate) fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count()
    }
}

pub(crate) fn byte_index_after_line(content: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return Some(0);
    }
    let mut seen = 0usize;
    for (index, ch) in content.char_indices() {
        if ch == '\n' {
            seen += 1;
            if seen == line {
                return Some(index + 1);
            }
        }
    }
    if line == count_lines(content) {
        return Some(content.len());
    }
    None
}

pub(crate) fn line_start_offset(content: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return None;
    }
    if line == 1 {
        return Some(0);
    }

    let mut current_line = 1usize;
    for (index, byte) in content.as_bytes().iter().enumerate() {
        if *byte == b'\n' {
            current_line += 1;
            if current_line == line {
                return Some(index + 1);
            }
        }
    }
    None
}

pub(crate) fn read_utf8_file_with_limit(
    path: &Path,
    display_path: &str,
    max_bytes: usize,
) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("Failed to open file '{}': {e}", display_path))?;
    let mut bytes = Vec::new();
    let mut limited = Read::by_ref(&mut file).take((max_bytes + 1) as u64);
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read file '{}': {e}", display_path))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "File too large: '{}' exceeds {} bytes",
            display_path, max_bytes
        ));
    }
    String::from_utf8(bytes).map_err(|e| format!("File '{}' is not valid UTF-8: {e}", display_path))
}

pub(crate) fn read_file_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .ok_or("read_file requires a string field: path")?;
    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", path))?;
    if !metadata.is_file() {
        return Err(format!("Path is not a file: '{}'", path));
    }
    let content = read_utf8_file_with_limit(&resolved_path, path, MAX_READ_FILE_BYTES)?;
    Ok(format!("Path: {}\n{}", path, content))
}

pub(crate) fn list_files_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, String> {
    let input_object = input
        .as_object()
        .ok_or("list_files requires an object input")?;
    let raw_path = match input_object.get("path") {
        None => ".",
        Some(value) => value
            .as_str()
            .ok_or("list_files requires an optional string field: path")?,
    };
    let path = if raw_path.is_empty() { "." } else { raw_path };
    let recursive =
        optional_bool_field(input_object, "recursive", "list_files recursive")?.unwrap_or(false);
    let glob = optional_string_field(input_object, "glob", "list_files glob")?;

    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect path '{}': {e}", path))?;

    if metadata.is_file() {
        if let Some(glob) = glob {
            let file_name = Path::new(path)
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("Failed to inspect file name for '{}'", path))?;
            if !glob_matches(glob, file_name) {
                return Ok(format!("Path: {path}\nGlob: {glob}\n(empty)"));
            }
        }
        return Ok(format!("Path: {}\n[file]", path));
    }

    if !metadata.is_dir() {
        return Err(format!("Path is neither a file nor directory: '{}'", path));
    }

    let entries = if recursive || glob.is_some() {
        collect_filtered_file_entries(&resolved_path, glob, recursive)?
    } else {
        collect_directory_entries(&resolved_path)?
    };
    format_list_file_entries(path, glob, recursive, entries)
}

fn collect_directory_entries(resolved_path: &Path) -> Result<Vec<(String, String)>, String> {
    let read_dir = fs::read_dir(resolved_path).map_err(|e| {
        format!(
            "Failed to list directory '{}': {e}",
            resolved_path.display()
        )
    })?;
    let mut entries = Vec::new();

    for entry_result in read_dir {
        let entry = entry_result.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to inspect directory entry type: {e}"))?;
        let (sort_key, line) = if file_type.is_symlink() {
            (name.clone(), format!("[lnk] {name}"))
        } else if file_type.is_dir() {
            (name.clone(), format!("[dir] {name}/"))
        } else {
            (name.clone(), format!("[file] {name}"))
        };
        entries.push((sort_key, line));
    }

    Ok(entries)
}

fn collect_filtered_file_entries(
    resolved_path: &Path,
    glob: Option<&str>,
    recursive: bool,
) -> Result<Vec<(String, String)>, String> {
    let mut entries = Vec::new();
    collect_filtered_file_entries_inner(
        resolved_path,
        resolved_path,
        glob,
        recursive,
        &mut entries,
    )?;
    Ok(entries)
}

fn collect_filtered_file_entries_inner(
    root: &Path,
    current: &Path,
    glob: Option<&str>,
    recursive: bool,
    entries: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let read_dir = fs::read_dir(current)
        .map_err(|e| format!("Failed to list directory '{}': {e}", current.display()))?;

    for entry_result in read_dir {
        let entry = entry_result.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to inspect directory entry type: {e}"))?;
        let path = entry.path();
        if file_type.is_dir() {
            if recursive {
                collect_filtered_file_entries_inner(root, &path, glob, recursive, entries)?;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .map_err(|e| format!("Failed to normalize path '{}': {e}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let matches = glob.is_none_or(|pattern| {
            glob_matches(pattern, &relative)
                || Path::new(&relative)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|file_name| glob_matches(pattern, file_name))
        });
        if matches {
            entries.push((relative.clone(), format!("[file] {relative}")));
        }
    }

    Ok(())
}

fn format_list_file_entries(
    path: &str,
    glob: Option<&str>,
    recursive: bool,
    mut entries: Vec<(String, String)>,
) -> Result<String, String> {
    if entries.is_empty() {
        let mut lines = vec![format!("Path: {path}")];
        if let Some(glob) = glob {
            lines.push(format!("Glob: {glob}"));
        }
        if recursive {
            lines.push("Recursive: true".to_string());
        }
        lines.push("(empty)".to_string());
        return Ok(lines.join("\n"));
    }

    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let has_more = entries.len() > MAX_LIST_ENTRIES;
    let lines = entries
        .into_iter()
        .take(MAX_LIST_ENTRIES)
        .map(|(_, line)| line)
        .collect::<Vec<_>>();
    let mut output = vec![format!("Path: {path}")];
    if let Some(glob) = glob {
        output.push(format!("Glob: {glob}"));
    }
    if recursive {
        output.push("Recursive: true".to_string());
    }
    output.push(lines.join("\n"));
    if has_more {
        output.push(format!(
            "(truncated: showing first {MAX_LIST_ENTRIES} entries; more entries omitted)"
        ));
    }
    Ok(output.join("\n"))
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    glob_match_bytes(pattern.as_bytes(), value.as_bytes())
}

fn glob_match_bytes(pattern: &[u8], value: &[u8]) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }

    match pattern[0] {
        b'*' => {
            glob_match_bytes(&pattern[1..], value)
                || (!value.is_empty() && glob_match_bytes(pattern, &value[1..]))
        }
        b'?' => !value.is_empty() && glob_match_bytes(&pattern[1..], &value[1..]),
        byte => {
            !value.is_empty() && byte == value[0] && glob_match_bytes(&pattern[1..], &value[1..])
        }
    }
}

pub(crate) fn resolve_workspace_command_cwd(
    workspace_root: &Path,
    requested_path: &str,
) -> Result<PathBuf, String> {
    let resolved = resolve_workspace_path(workspace_root, requested_path)?;
    let metadata = fs::metadata(&resolved)
        .map_err(|e| format!("Failed to inspect command cwd '{}': {e}", requested_path))?;
    if !metadata.is_dir() {
        return Err(format!(
            "run_command cwd is not a directory: '{}'",
            requested_path
        ));
    }
    Ok(resolved)
}

pub(crate) fn resolve_workspace_path(
    workspace_root: &Path,
    requested_path: &str,
) -> Result<PathBuf, String> {
    let workspace_canonical = workspace_root.canonicalize().map_err(|e| {
        format!(
            "Failed to resolve workspace root '{}': {e}",
            workspace_root.display()
        )
    })?;
    let path = PathBuf::from(requested_path);
    if path.is_absolute() {
        return Err(format!(
            "Absolute paths are not allowed: '{}'",
            requested_path
        ));
    }
    let joined = workspace_canonical.join(path);
    let canonical = joined
        .canonicalize()
        .map_err(|e| format!("Failed to resolve path '{}': {e}", requested_path))?;
    if !canonical.starts_with(&workspace_canonical) {
        return Err(format!(
            "Path '{}' is outside the workspace",
            requested_path
        ));
    }
    Ok(canonical)
}

pub(crate) fn resolve_workspace_path_for_create(
    workspace_root: &Path,
    requested_path: &str,
) -> Result<PathBuf, String> {
    let workspace_canonical = workspace_root.canonicalize().map_err(|e| {
        format!(
            "Failed to resolve workspace root '{}': {e}",
            workspace_root.display()
        )
    })?;
    let path = PathBuf::from(requested_path);
    if path.is_absolute() {
        return Err(format!(
            "Absolute paths are not allowed: '{}'",
            requested_path
        ));
    }
    let joined = workspace_canonical.join(path);
    match fs::symlink_metadata(&joined) {
        Ok(metadata) => {
            let canonical = match joined.canonicalize() {
                Ok(path) => path,
                Err(e) if metadata.file_type().is_symlink() && e.kind() == ErrorKind::NotFound => {
                    return Err(format!(
                        "Path '{}' points to a dangling symlink",
                        requested_path
                    ));
                }
                Err(e) => return Err(format!("Failed to resolve path '{}': {e}", requested_path)),
            };
            if !canonical.starts_with(&workspace_canonical) {
                return Err(format!(
                    "Path '{}' is outside the workspace",
                    requested_path
                ));
            }
            return Ok(canonical);
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!("Failed to inspect path '{}': {e}", requested_path));
        }
    }

    let mut canonical_ancestor: Option<PathBuf> = None;
    for ancestor in joined.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                let canonical = match ancestor.canonicalize() {
                    Ok(path) => path,
                    Err(e)
                        if metadata.file_type().is_symlink() && e.kind() == ErrorKind::NotFound =>
                    {
                        return Err(format!(
                            "Path '{}' includes a dangling symlink in an ancestor",
                            requested_path
                        ));
                    }
                    Err(e) => {
                        return Err(format!(
                            "Failed to resolve parent directory for '{}': {e}",
                            requested_path
                        ));
                    }
                };
                canonical_ancestor = Some(canonical);
                break;
            }
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to inspect parent directory for '{}': {e}",
                    requested_path
                ));
            }
        }
    }

    let canonical_ancestor = canonical_ancestor.ok_or_else(|| {
        format!(
            "Failed to resolve parent directory for path '{}'",
            requested_path
        )
    })?;
    if !canonical_ancestor.starts_with(&workspace_canonical) {
        return Err(format!(
            "Path '{}' is outside the workspace",
            requested_path
        ));
    }
    Ok(joined)
}
