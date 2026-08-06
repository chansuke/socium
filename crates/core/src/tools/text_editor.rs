use crate::AgentProgress;
use crate::constants::*;
use crate::tools::edit_history::{pop_edit_history, record_edit_history};
use crate::tools::error::ToolError;
use crate::tools::git::maybe_prepend_dirty_worktree_warning;
use crate::tools::input::{require_i64_field, require_string_field};
use crate::tools::list_files::list_files_from_workspace;
use crate::tools::text_file::{
    apply_view_range, byte_index_after_line, count_lines, read_utf8_file_with_limit,
    truncate_by_characters,
};
use crate::tools::workspace::{resolve_workspace_path, resolve_workspace_path_for_create};
use crate::tools::{AnthropicTextEditorToolDefinition, Tool, ToolContext, ToolDefinition};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

pub(crate) struct TextEditorTool;

impl Tool for TextEditorTool {
    fn name(&self) -> &'static str {
        TEXT_EDITOR_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::AnthropicTextEditor(AnthropicTextEditorToolDefinition {
            tool_type: "text_editor_20250728".to_string(),
            name: TEXT_EDITOR_TOOL_NAME.to_string(),
            max_characters: Some(MAX_TEXT_EDITOR_VIEW_CHARACTERS),
        })
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        _on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        text_editor_from_workspace(ctx.workspace_root, input)
    }
}

pub(crate) fn text_editor_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    let input_object = input
        .as_object()
        .ok_or("text_editor tool requires an object input")?;
    let command = require_string_field(input_object, "command", "text_editor command")?;

    match command {
        "view" => text_editor_view(workspace_root, input_object),
        "str_replace" => text_editor_str_replace(workspace_root, input_object),
        "create" => text_editor_create(workspace_root, input_object),
        "insert" => text_editor_insert(workspace_root, input_object),
        "undo_edit" => text_editor_undo_edit(workspace_root, input_object),
        other => Err(format!("Unsupported text editor command: {other}").into()),
    }
}

pub(crate) fn text_editor_view(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, ToolError> {
    let path = require_string_field(input, "path", "text_editor view path")?;
    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect path '{}': {e}", path))?;

    if metadata.is_dir() {
        return list_files_from_workspace(workspace_root, &json!({ "path": path }));
    }
    if !metadata.is_file() {
        return Err(format!("Path is neither a file nor directory: '{}'", path).into());
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
) -> Result<String, ToolError> {
    let path = require_string_field(input, "path", "text_editor str_replace path")?;
    let old_str = require_string_field(input, "old_str", "text_editor str_replace old_str")?;
    let new_str = require_string_field(input, "new_str", "text_editor str_replace new_str")?;
    if old_str.is_empty() {
        return Err("text_editor str_replace old_str cannot be empty".into());
    }

    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", path))?;
    if !metadata.is_file() {
        return Err(format!("Path is not a file: '{}'", path).into());
    }

    let content = read_utf8_file_with_limit(&resolved_path, path, MAX_READ_FILE_BYTES)?;
    let matches = content.match_indices(old_str).count();
    if matches == 0 {
        return Err(format!(
            "No matches found for old_str in file '{}'; replacement not applied",
            path
        )
        .into());
    }
    if matches > 1 {
        return Err(format!(
            "Multiple matches found for old_str in file '{}'; provide a more specific old_str",
            path
        )
        .into());
    }

    let updated = content.replacen(old_str, new_str, 1);
    record_edit_history(workspace_root, path, Some(content))?;
    fs::write(&resolved_path, updated)
        .map_err(|e| format!("Failed to write file '{}': {e}", path))?;
    Ok(maybe_prepend_dirty_worktree_warning(
        workspace_root,
        format!("Path: {}\nReplaced 1 occurrence", path),
    ))
}

pub(crate) fn text_editor_create(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, ToolError> {
    let path = require_string_field(input, "path", "text_editor create path")?;
    let file_text = require_string_field(input, "file_text", "text_editor create file_text")?;
    let target_path = resolve_workspace_path_for_create(workspace_root, path)?;

    if target_path.exists() {
        return Err(format!("Cannot create file '{}': path already exists", path).into());
    }
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directories for '{}': {e}", path))?;
    }
    record_edit_history(workspace_root, path, None)?;
    fs::write(&target_path, file_text)
        .map_err(|e| format!("Failed to create file '{}': {e}", path))?;
    Ok(maybe_prepend_dirty_worktree_warning(
        workspace_root,
        format!("Path: {}\nCreated file", path),
    ))
}

pub(crate) fn text_editor_insert(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, ToolError> {
    let path = require_string_field(input, "path", "text_editor insert path")?;
    let insert_line = require_i64_field(input, "insert_line", "text_editor insert insert_line")?;
    if insert_line < 0 {
        return Err("text_editor insert insert_line must be >= 0".into());
    }

    let insert_text = require_string_field(input, "insert_text", "text_editor insert insert_text")?;

    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", path))?;
    if !metadata.is_file() {
        return Err(format!("Path is not a file: '{}'", path).into());
    }

    let content = read_utf8_file_with_limit(&resolved_path, path, MAX_READ_FILE_BYTES)?;
    let line_count = count_lines(&content);
    let line = insert_line as usize;
    if line > line_count {
        return Err(format!(
            "insert_line {} is out of range for '{}' (file has {} lines)",
            line, path, line_count
        )
        .into());
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

    record_edit_history(workspace_root, path, Some(content))?;
    fs::write(&resolved_path, updated)
        .map_err(|e| format!("Failed to write file '{}': {e}", path))?;
    Ok(maybe_prepend_dirty_worktree_warning(
        workspace_root,
        format!("Path: {}\nInserted text after line {}", path, line),
    ))
}

pub(crate) fn text_editor_undo_edit(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, ToolError> {
    let path = require_string_field(input, "path", "text_editor undo_edit path")?;
    let history_entry = pop_edit_history(workspace_root, path)?
        .ok_or_else(|| format!("No edit history found for '{}'", path))?;
    let target_path = resolve_workspace_path_for_create(workspace_root, path)?;

    match history_entry.previous_content {
        Some(previous_content) => {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!("Failed to create parent directories for '{}': {e}", path)
                })?;
            }
            fs::write(&target_path, previous_content)
                .map_err(|e| format!("Failed to restore file '{}': {e}", path))?;
        }
        None => {
            if target_path.exists() {
                fs::remove_file(&target_path)
                    .map_err(|e| format!("Failed to remove file '{}': {e}", path))?;
            }
        }
    }

    Ok(maybe_prepend_dirty_worktree_warning(
        workspace_root,
        format!("Path: {}\nUndid last edit", path),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::agent::types::ContentBlock;
    use crate::test_support::{init_git_repo, make_temp_dir};
    use crate::tools::run_tool_call;
    use serde_json::json;

    #[test]
    fn text_editor_create_warns_when_git_worktree_is_dirty() {
        let workspace = make_temp_dir("git-dirty-create");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "dirty\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_dirty_create",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "create",
                "path": "new.txt",
                "file_text": "created"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Created file"));
                assert!(content.contains("Warning: git worktree has uncommitted changes"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_view_rejects_invalid_negative_end_line() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("edit.txt"), "first\nsecond\nthird\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_view_neg_end",
            TEXT_EDITOR_TOOL_NAME,
            &json!({"command": "view", "path": "edit.txt", "view_range": [1, -2]}),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("end must be -1 or a positive integer"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_view_range_preserves_line_endings() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("crlf.txt");
        fs::write(&file_path, b"line1\r\nline2\r\nline3\r\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_view_crlf",
            TEXT_EDITOR_TOOL_NAME,
            &json!({"command": "view", "path": "crlf.txt", "view_range": [2, 2]}),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert_eq!(content, "Path: crlf.txt\nline2\r\n");
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_view_large_file_is_truncated_instead_of_rejected() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("large-view.txt");
        let large = vec![b'a'; MAX_READ_FILE_BYTES + 1];
        fs::write(&file_path, large).unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_view_large",
            TEXT_EDITOR_TOOL_NAME,
            &json!({"command": "view", "path": "large-view.txt"}),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Path: large-view.txt"));
                assert!(content.contains("(truncated: showing first"));
                assert!(!content.contains("File too large"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_view_rejects_extremely_large_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("too-large-view.txt");
        let huge = vec![b'a'; MAX_TEXT_EDITOR_VIEW_READ_BYTES + 1];
        fs::write(&file_path, huge).unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_view_too_large",
            TEXT_EDITOR_TOOL_NAME,
            &json!({"command": "view", "path": "too-large-view.txt"}),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("File too large"));
                assert!(content.contains("too-large-view.txt"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_str_replace_updates_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("replace.txt");
        fs::write(&file_path, "hello old world").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_2",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "str_replace",
                "path": "replace.txt",
                "old_str": "old",
                "new_str": "new"
            }),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }
        let updated = fs::read_to_string(&file_path).unwrap();
        assert_eq!(updated, "hello new world");
    }

    #[test]
    fn text_editor_str_replace_rejects_multiple_matches() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("replace.txt"), "dup\ndup\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_3",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "str_replace",
                "path": "replace.txt",
                "old_str": "dup",
                "new_str": "x"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("Multiple matches found"));
                assert_eq!(is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_create_creates_new_file() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_te_4",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "create",
                "path": "new.txt",
                "file_text": "created"
            }),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }
        let content = fs::read_to_string(workspace.path().join("new.txt")).unwrap();
        assert_eq!(content, "created");
    }

    #[test]
    fn text_editor_create_creates_missing_parent_directories() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_te_create_nested",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "create",
                "path": "newdir/inner/new.txt",
                "file_text": "created"
            }),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }
        let content = fs::read_to_string(workspace.path().join("newdir/inner/new.txt")).unwrap();
        assert_eq!(content, "created");
    }

    #[test]
    fn text_editor_create_rejects_existing_path() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("exists.txt"), "keep").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_5",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "create",
                "path": "exists.txt",
                "file_text": "overwrite"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("already exists"));
                assert_eq!(is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_insert_inserts_after_line() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("insert.txt");
        fs::write(&file_path, "a\nb").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_6",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "insert",
                "path": "insert.txt",
                "insert_line": 1,
                "insert_text": "X\n"
            }),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }
        let updated = fs::read_to_string(&file_path).unwrap();
        assert_eq!(updated, "a\nX\nb");
    }

    #[test]
    fn text_editor_insert_rejects_out_of_range_line() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("insert.txt"), "a\nb").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_7",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "insert",
                "path": "insert.txt",
                "insert_line": 3,
                "insert_text": "X\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("out of range"));
                assert_eq!(is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_insert_requires_insert_text_field() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("insert.txt"), "a\nb").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_insert_missing_text",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "insert",
                "path": "insert.txt",
                "insert_line": 1,
                "new_str": "X\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("requires a string field: insert_text"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_undo_edit_restores_previous_file_content() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("undo.txt");
        fs::write(&file_path, "before").unwrap();

        let edit_result = run_tool_call(
            workspace.path(),
            "tool_te_undo_edit_before",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "str_replace",
                "path": "undo.txt",
                "old_str": "before",
                "new_str": "after"
            }),
        );

        match edit_result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }

        let result = run_tool_call(
            workspace.path(),
            "tool_te_8",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "undo_edit",
                "path": "undo.txt"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("Undid last edit"));
                assert_eq!(is_error, None);
            }
            _ => panic!("expected tool_result"),
        }

        let restored = fs::read_to_string(&file_path).unwrap();
        assert_eq!(restored, "before");
    }

    #[test]
    fn text_editor_undo_edit_removes_last_created_file() {
        let workspace = make_temp_dir("workspace");

        let create_result = run_tool_call(
            workspace.path(),
            "tool_te_undo_create_before",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "create",
                "path": "created.txt",
                "file_text": "new"
            }),
        );

        match create_result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }

        let undo_result = run_tool_call(
            workspace.path(),
            "tool_te_undo_create",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "undo_edit",
                "path": "created.txt"
            }),
        );

        match undo_result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("Undid last edit"));
                assert_eq!(is_error, None);
            }
            _ => panic!("expected tool_result"),
        }

        assert!(!workspace.path().join("created.txt").exists());
    }
}
