use crate::ContentBlock;
use crate::constants::*;
use serde_json::{Value, json};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

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

    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect path '{}': {e}", path))?;

    if metadata.is_file() {
        return Ok(format!("Path: {}\n[file]", path));
    }

    if !metadata.is_dir() {
        return Err(format!("Path is neither a file nor directory: '{}'", path));
    }

    let read_dir = fs::read_dir(&resolved_path)
        .map_err(|e| format!("Failed to list directory '{}': {e}", path))?;
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut has_more = false;

    for entry_result in read_dir {
        let entry = entry_result.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to inspect directory entry type: {e}"))?;
        let (prefix, display_name) = if file_type.is_symlink() {
            ("[lnk]", name.clone())
        } else if file_type.is_dir() {
            ("[dir]", format!("{name}/"))
        } else {
            ("[file]", name.clone())
        };
        entries.push((name, format!("{prefix} {display_name}")));

        if entries.len() > MAX_LIST_ENTRIES {
            has_more = true;
            break;
        }
    }

    if entries.is_empty() {
        return Ok(format!("Path: {}\n(empty)", path));
    }

    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    if has_more {
        let lines: Vec<String> = entries
            .iter()
            .take(MAX_LIST_ENTRIES)
            .map(|(_, line)| line.clone())
            .collect();
        Ok(format!(
            "Path: {}\n{}\n(truncated: showing first {MAX_LIST_ENTRIES} entries; more entries omitted)",
            path,
            lines.join("\n")
        ))
    } else {
        let lines: Vec<String> = entries.into_iter().map(|(_, line)| line).collect();
        Ok(format!("Path: {}\n{}", path, lines.join("\n")))
    }
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
