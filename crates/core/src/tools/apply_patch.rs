use crate::AgentProgress;
use crate::constants::*;
use crate::tools::error::ToolError;
use crate::tools::git::maybe_prepend_dirty_worktree_warning;
use crate::tools::input::{optional_bool_field, require_string_field};
use crate::tools::text_file::read_utf8_file_with_limit;
use crate::tools::workspace::{resolve_workspace_path, resolve_workspace_path_for_create};
use crate::tools::{CustomToolDefinition, Tool, ToolContext, ToolDefinition};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

pub(crate) struct ApplyPatchTool;

impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::Custom(CustomToolDefinition {
            name: "apply_patch".to_string(),
            description: "Apply a structured patch with add, delete, or multi-hunk update operations inside the workspace."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dry_run": {
                        "type": "boolean",
                        "description": "If true, validates the patch and returns a preview without writing files."
                    },
                    "patch": {
                        "type": "string",
                        "description": "Patch text using *** Begin Patch / *** End Patch blocks with Add File, Delete File, or Update File sections."
                    }
                },
                "required": ["patch"],
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
        apply_patch_from_workspace(ctx.workspace_root, input)
    }
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

struct ApplyPatchInput<'a> {
    patch: &'a str,
    dry_run: bool,
}

impl<'a> ApplyPatchInput<'a> {
    fn parse(input: &'a Value) -> Result<Self, ToolError> {
        let input_object = input
            .as_object()
            .ok_or("apply_patch requires an object input")?;
        let patch = require_string_field(input_object, "patch", "apply_patch")?;
        let dry_run =
            optional_bool_field(input_object, "dry_run", "apply_patch dry_run")?.unwrap_or(false);
        Ok(Self { patch, dry_run })
    }
}

pub(crate) fn apply_patch_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    let ApplyPatchInput { patch, dry_run } = ApplyPatchInput::parse(input)?;
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
                    return Err(format!("Cannot add file '{}': path already exists", path).into());
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
                    return Err(format!("Path is not a file: '{}'", path).into());
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
                    return Err(format!("Path is not a file: '{}'", path).into());
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
                        )
                        .into());
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

    Ok(maybe_prepend_dirty_worktree_warning(
        workspace_root,
        results.join("\n"),
    ))
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

fn preview_patch_actions(
    workspace_root: &Path,
    actions: &[PatchAction],
) -> Result<String, ToolError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::ContentBlock;
    use crate::test_support::{init_git_repo, make_temp_dir};
    use crate::tools::run_tool_call;
    use serde_json::json;

    #[test]
    fn apply_patch_updates_multiple_hunks_in_one_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("notes.txt");
        fs::write(&file_path, "one\ntwo\nthree\nfour\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_update",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Update File: notes.txt\n@@\n one\n-two\n+TWO\n three\n@@\n three\n-four\n+FOUR\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Updated file: notes.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        let updated = fs::read_to_string(&file_path).unwrap();
        assert_eq!(updated, "one\nTWO\nthree\nFOUR\n");
    }

    #[test]
    fn apply_patch_creates_new_file() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_add",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Add File: created.txt\n+hello\n+world\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Added file: created.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        let created = fs::read_to_string(workspace.path().join("created.txt")).unwrap();
        assert_eq!(created, "hello\nworld");
    }

    #[test]
    fn apply_patch_deletes_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("remove.txt");
        fs::write(&file_path, "delete me").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_delete",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Delete File: remove.txt\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Deleted file: remove.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        assert!(!file_path.exists());
    }

    #[test]
    fn apply_patch_moves_file_and_updates_contents() {
        let workspace = make_temp_dir("workspace");
        let from_path = workspace.path().join("old.txt");
        fs::write(&from_path, "alpha\nbeta\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_move",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n alpha\n-beta\n+gamma\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Moved file: old.txt -> new.txt"));
                assert!(content.contains("Updated file: new.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        assert!(!from_path.exists());
        let moved = fs::read_to_string(workspace.path().join("new.txt")).unwrap();
        assert_eq!(moved, "alpha\ngamma\n");
    }

    #[test]
    fn apply_patch_dry_run_returns_preview_without_writing() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("preview.txt");
        fs::write(&file_path, "before\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_preview",
            "apply_patch",
            &json!({
                "dry_run": true,
                "patch": "*** Begin Patch\n*** Update File: preview.txt\n@@\n-before\n+after\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Preview: true"));
                assert!(content.contains("--- preview.txt"));
                assert!(content.contains("+++ preview.txt"));
                assert!(content.contains("-before"));
                assert!(content.contains("+after"));
            }
            _ => panic!("expected tool_result"),
        }

        let unchanged = fs::read_to_string(&file_path).unwrap();
        assert_eq!(unchanged, "before\n");
    }

    #[test]
    fn apply_patch_warns_when_git_worktree_is_dirty() {
        let workspace = make_temp_dir("git-dirty-patch");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "dirty\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_dirty_patch",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Add File: another.txt\n+hello\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Added file: another.txt"));
                assert!(content.contains("Warning: git worktree has uncommitted changes"));
            }
            _ => panic!("expected tool_result"),
        }
    }
}
