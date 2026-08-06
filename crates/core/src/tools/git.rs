use crate::AgentProgress;
use crate::tools::error::ToolError;
use crate::tools::input::{
    optional_bool_field, optional_bounded_usize_field, optional_string_field, require_object_input,
    require_string_field,
};
use crate::tools::workspace::resolve_workspace_path;
use crate::tools::{CustomToolDefinition, Tool, ToolContext, ToolDefinition};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;

pub(crate) struct GitBranchTool;

impl Tool for GitBranchTool {
    fn name(&self) -> &'static str {
        "git_branch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::Custom(CustomToolDefinition {
            name: "git_branch".to_string(),
            description: "Show the current Git branch for the workspace repository.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        })
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
        ToolDefinition::Custom(CustomToolDefinition {
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
        })
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
        ToolDefinition::Custom(CustomToolDefinition {
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
        })
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        _on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        git_diff_from_workspace(ctx.workspace_root, input)
    }
}

pub(crate) struct GitLogTool;

impl Tool for GitLogTool {
    fn name(&self) -> &'static str {
        "git_log"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::Custom(CustomToolDefinition {
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
        })
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
        ToolDefinition::Custom(CustomToolDefinition {
            name: "git_commit".to_string(),
            description: "Create a Git commit for the current workspace, but only when the caller explicitly confirms the intent."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "Commit message to use."
                    },
                    "confirmed": {
                        "type": "boolean",
                        "description": "Must be true to confirm explicit user intent for the commit."
                    }
                },
                "required": ["message", "confirmed"],
                "additionalProperties": false
            }),
        })
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        let mut forward = |progress| on_progress(progress);
        git_commit_from_workspace_with_progress(ctx.workspace_root, input, &mut forward)
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

struct GitCommitInput<'a> {
    message: &'a str,
    confirmed: bool,
}

impl<'a> GitCommitInput<'a> {
    fn parse(input: &'a Value) -> Result<Self, ToolError> {
        let input_object = require_object_input(input, "git_commit")?;
        let message = require_string_field(input_object, "message", "git_commit")?;
        let confirmed = optional_bool_field(input_object, "confirmed", "git_commit confirmed")?
            .unwrap_or(false);
        Ok(Self { message, confirmed })
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

pub(crate) fn git_diff_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    let GitPathInput { path } = GitPathInput::parse(input, "git_diff", "git_diff path")?;
    let mut args = vec!["diff".to_string()];
    if let Some(path) = path {
        args.push("--".to_string());
        args.push(git_relative_path(workspace_root, path)?);
    }
    let output = run_git_capture(workspace_root, &args)?;
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

#[cfg(test)]
pub(crate) fn git_commit_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    let mut noop = |_| {};
    git_commit_from_workspace_with_progress(workspace_root, input, &mut noop)
}

pub(crate) fn git_commit_from_workspace_with_progress<F>(
    workspace_root: &Path,
    input: &Value,
    on_progress: &mut F,
) -> Result<String, ToolError>
where
    F: FnMut(AgentProgress),
{
    let GitCommitInput { message, confirmed } = GitCommitInput::parse(input)?;
    if !confirmed {
        on_progress(AgentProgress::ApprovalRequired {
            tool: "git_commit".to_string(),
            reason: "explicit confirmation required".to_string(),
        });
        return Err(
            "git_commit requires explicit confirmation. Rerun with confirmed set to true.".into(),
        );
    }

    let status_output = run_git_capture(workspace_root, &["status", "--porcelain"])?;
    if status_output.stdout.is_empty() {
        return Err("git_commit found no changes to commit".into());
    }

    run_git_capture(workspace_root, &["add", "-A"])?;
    run_git_capture(
        workspace_root,
        &["commit".to_string(), "-m".to_string(), message.to_string()],
    )?;
    let head = run_git_capture(workspace_root, &["rev-parse", "--short", "HEAD"])?;
    let commit = stdout_to_string(&head)?.trim().to_string();

    Ok(format!("Commit: {commit}\nMessage: {message}"))
}

fn run_git_capture<S>(workspace_root: &Path, args: &[S]) -> Result<std::process::Output, String>
where
    S: AsRef<str> + std::fmt::Debug,
{
    let output = Command::new("git")
        .args(args.iter().map(AsRef::as_ref))
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

pub(crate) fn maybe_prepend_dirty_worktree_warning(
    workspace_root: &Path,
    content: String,
) -> String {
    if git_worktree_is_dirty(workspace_root) {
        format!("Warning: git worktree has uncommitted changes\n{content}")
    } else {
        content
    }
}

fn git_worktree_is_dirty(workspace_root: &Path) -> bool {
    let Ok(output) = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(workspace_root)
        .output()
    else {
        return false;
    };
    output.status.success() && !output.stdout.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::ContentBlock;
    use crate::test_support::{git_stdout, init_git_repo, make_temp_dir};
    use crate::tools::{run_tool_call, run_tool_call_with_progress};
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
    fn git_log_returns_recent_commits() {
        let workspace = make_temp_dir("git-log");
        init_git_repo(workspace.path());

        let output = git_log_from_workspace(workspace.path(), &json!({ "max_count": 1 })).unwrap();

        assert!(output.contains("Count: 1"));
        assert!(output.contains("initial commit"));
    }

    #[test]
    fn git_commit_requires_explicit_confirmation() {
        let workspace = make_temp_dir("git-commit-confirm");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_git_commit_denied",
            "git_commit",
            &json!({
                "message": "save work"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("requires explicit confirmation"));
            }
            _ => panic!("expected tool_result"),
        }

        let head_message = git_stdout(workspace.path(), &["log", "-1", "--pretty=%s"]);
        assert_eq!(head_message.trim(), "initial commit");
    }

    #[test]
    fn git_commit_creates_commit_when_confirmed() {
        let workspace = make_temp_dir("git-commit-ok");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();

        let output = git_commit_from_workspace(
            workspace.path(),
            &json!({
                "message": "save work",
                "confirmed": true
            }),
        )
        .unwrap();

        assert!(output.contains("Commit:"));
        assert!(output.contains("Message: save work"));

        let head_message = git_stdout(workspace.path(), &["log", "-1", "--pretty=%s"]);
        assert_eq!(head_message.trim(), "save work");

        let status = git_stdout(workspace.path(), &["status", "--short"]);
        assert!(status.trim().is_empty());
    }

    #[test]
    fn git_commit_emits_approval_required_event_when_confirmation_is_missing() {
        let workspace = make_temp_dir("git-commit-needs-confirmation");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();

        let mut progress = Vec::new();
        let result = run_tool_call_with_progress(
            workspace.path(),
            "tool_git_commit_needs_confirmation",
            "git_commit",
            &json!({
                "message": "test commit"
            }),
            &mut |event| progress.push(event),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, Some(true)),
            _ => panic!("expected tool_result"),
        }
        assert_eq!(
            progress,
            vec![
                AgentProgress::ToolCallStarted {
                    name: "git_commit".to_string(),
                },
                AgentProgress::ApprovalRequired {
                    tool: "git_commit".to_string(),
                    reason: "explicit confirmation required".to_string(),
                },
                AgentProgress::ToolCallFinished {
                    name: "git_commit".to_string(),
                    is_error: true,
                },
            ]
        );
    }
}
