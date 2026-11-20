use crate::AgentProgress;
use crate::constants::*;
use crate::conversation::ToolDefinition;
use crate::tools::error::ToolError;
use crate::tools::text_file::read_utf8_file_with_limit;
use crate::tools::workspace::resolve_workspace_path;
use crate::tools::{Tool, ToolContext};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

pub(crate) struct ReadFileTool;

impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: format!(
                "Read a UTF-8 text file from the current workspace by relative path. Lines come numbered like `cat -n` (line number, tab, text); cite them as path:line. Returns up to {READ_FILE_DEFAULT_LINES} lines from offset; a longer file says where to continue."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to a file in the workspace. Absolute paths are not allowed."
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "First line to read, from 1. Defaults to 1."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": format!("Lines to read. Defaults to {READ_FILE_DEFAULT_LINES}.")
                    }
                },
                "required": ["path"],
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
        read_file_from_workspace(ctx.workspace_root, input)
    }
}

struct ReadFileInput<'a> {
    path: &'a str,
    offset: usize,
    limit: usize,
}

impl<'a> ReadFileInput<'a> {
    fn parse(input: &'a Value) -> Result<Self, ToolError> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or("read_file requires a string field: path")?;
        let positive = |field: &str, default: usize| match input.get(field) {
            None | Some(Value::Null) => Ok(default),
            Some(value) => value
                .as_u64()
                .filter(|n| *n >= 1)
                .map(|n| n as usize)
                .ok_or_else(|| format!("read_file {field} must be a positive integer")),
        };
        Ok(Self {
            path,
            offset: positive("offset", 1)?,
            limit: positive("limit", READ_FILE_DEFAULT_LINES)?,
        })
    }
}

/// A command's saved stdout or stderr, `.socium/outputs/<stem>.stdout`.
fn is_command_artifact(path: &str) -> bool {
    path.strip_prefix(".socium/outputs/")
        .and_then(|name| {
            name.strip_suffix(".stdout")
                .or_else(|| name.strip_suffix(".stderr"))
        })
        .is_some_and(|stem| {
            !stem.is_empty() && stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

pub(crate) fn read_file_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    let input = ReadFileInput::parse(input)?;
    let artifact = is_command_artifact(input.path);
    let resolved_path = match resolve_workspace_path(workspace_root, input.path) {
        Err(_) if artifact && !workspace_root.join(input.path).exists() => {
            return Err(format!(
                "The saved output '{}' is no longer kept: Socium keeps the output of the newest {MAX_KEPT_COMMAND_OUTPUTS} commands. Run the command again to see it.",
                input.path
            )
            .into());
        }
        resolved => resolved?,
    };
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", input.path))?;
    if !metadata.is_file() {
        return Err(format!("Path is not a file: '{}'", input.path).into());
    }
    let content = if artifact {
        // Saved command output can be larger than a source file and need
        // not be UTF-8; it is read whole and shown with replacements.
        let bytes = fs::read(&resolved_path)
            .map_err(|e| format!("Failed to read '{}': {e}", input.path))?;
        String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_COMMAND_ARTIFACT_BYTES as usize)])
            .into_owned()
    } else {
        read_utf8_file_with_limit(&resolved_path, input.path, MAX_TEXT_EDITOR_VIEW_READ_BYTES)?
    };
    let window = crate::tools::text_file::numbered_lines(
        &content,
        input.offset,
        input.limit,
        MAX_READ_OUTPUT_CHARACTERS,
    );
    if window.total == 0 {
        return Ok(format!("Path: {} (empty file)", input.path));
    }
    if input.offset > window.total {
        return Err(format!(
            "offset {} is past the end of '{}' ({} lines)",
            input.offset, input.path, window.total
        )
        .into());
    }
    let more = if window.last < window.total {
        format!("\n(more: continue with offset {})", window.last + 1)
    } else {
        String::new()
    };
    Ok(format!(
        "Path: {} (lines {}-{} of {})\n{}{more}",
        input.path, input.offset, window.last, window.total, window.text
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use serde_json::json;
    use std::fs;

    #[test]
    fn reads_file_from_workspace() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("note.txt");
        fs::write(&file_path, "hello").unwrap();

        let output =
            read_file_from_workspace(workspace.path(), &json!({"path": "note.txt"})).unwrap();

        assert!(output.contains("Path:"));
        assert!(output.contains("hello"));
    }

    #[test]
    fn rejects_path_outside_workspace() {
        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let file_path = outside.path().join("secret.txt");
        fs::write(&file_path, "secret").unwrap();

        let output = read_file_from_workspace(
            workspace.path(),
            &json!({"path": file_path.to_string_lossy().to_string()}),
        );

        assert!(output.is_err());
    }

    #[test]
    fn rejects_too_large_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("large.txt");
        let large = vec![b'a'; MAX_TEXT_EDITOR_VIEW_READ_BYTES + 1];
        fs::write(&file_path, large).unwrap();

        let output = read_file_from_workspace(workspace.path(), &json!({"path": "large.txt"}));

        assert!(output.is_err());
    }

    #[test]
    fn lines_are_numbered_and_large_files_are_read_in_pages() {
        // Review R10 and N3: no line numbers to cite, and a 200 KB file
        // came back whole.
        let workspace = make_temp_dir("workspace");
        let text = (1..=5_000)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(workspace.path().join("big.txt"), text).unwrap();

        let first =
            read_file_from_workspace(workspace.path(), &json!({"path": "big.txt"})).unwrap();
        assert!(first.starts_with("Path: big.txt (lines 1-2000 of 5000)\n     1\tline 1\n"));
        assert!(first.ends_with("  2000\tline 2000\n\n(more: continue with offset 2001)"));

        let last = read_file_from_workspace(
            workspace.path(),
            &json!({"path": "big.txt", "offset": 4999, "limit": 10}),
        )
        .unwrap();
        assert_eq!(
            last,
            "Path: big.txt (lines 4999-5000 of 5000)\n  4999\tline 4999\n  5000\tline 5000\n"
        );
        assert!(
            read_file_from_workspace(workspace.path(), &json!({"path": "big.txt", "offset": 0}))
                .is_err()
        );
        assert!(
            read_file_from_workspace(
                workspace.path(),
                &json!({"path": "big.txt", "offset": 6000})
            )
            .unwrap_err()
            .to_string()
            .contains("5000 lines")
        );
    }

    #[test]
    fn long_lines_and_long_pages_are_cut_with_a_note() {
        let workspace = make_temp_dir("workspace");
        let line = "x".repeat(MAX_LINE_CHARACTERS + 50);
        let text = vec![line; 100].join("\n");
        fs::write(workspace.path().join("min.js"), text).unwrap();

        let output =
            read_file_from_workspace(workspace.path(), &json!({"path": "min.js"})).unwrap();
        assert!(output.contains("[line cut at 2000 characters]"));
        assert!(output.chars().count() < MAX_READ_OUTPUT_CHARACTERS + 200);
        assert!(output.contains("(more: continue with offset"));
    }

    #[test]
    fn saved_command_output_reads_back_past_the_source_file_limit() {
        let workspace = make_temp_dir("artifact_read");
        let outputs = workspace.path().join(".socium/outputs");
        fs::create_dir_all(&outputs).unwrap();
        let mut bytes = Vec::new();
        for line in 1..=200_000 {
            bytes.extend_from_slice(format!("line {line} {}\n", "x".repeat(20)).as_bytes());
        }
        bytes.extend_from_slice(b"error: \xff broken\n");
        assert!(bytes.len() > MAX_TEXT_EDITOR_VIEW_READ_BYTES);
        fs::write(outputs.join("1-2-0.stderr"), &bytes).unwrap();

        let tail = read_file_from_workspace(
            workspace.path(),
            &json!({"path": ".socium/outputs/1-2-0.stderr", "offset": 200_001}),
        )
        .unwrap();
        assert!(tail.contains("(lines 200001-200001 of 200001)"), "{tail}");
        assert!(tail.contains("error: \u{fffd} broken"), "{tail}");

        let gone = read_file_from_workspace(
            workspace.path(),
            &json!({"path": ".socium/outputs/1-3-0.stdout"}),
        )
        .unwrap_err()
        .to_string();
        assert!(gone.contains("is no longer kept"), "{gone}");

        // Other state is not read past the ordinary limit.
        fs::write(workspace.path().join(".socium/big.json"), &bytes).unwrap();
        assert!(
            read_file_from_workspace(workspace.path(), &json!({"path": ".socium/big.json"}))
                .is_err()
        );
    }
}
