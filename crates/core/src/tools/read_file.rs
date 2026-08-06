use crate::constants::*;
use crate::tools::error::ToolError;
use crate::tools::text_file::read_utf8_file_with_limit;
use crate::tools::workspace::resolve_workspace_path;
use serde_json::Value;
use std::fs;
use std::path::Path;

struct ReadFileInput<'a> {
    path: &'a str,
}

impl<'a> ReadFileInput<'a> {
    fn parse(input: &'a Value) -> Result<Self, ToolError> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .ok_or("read_file requires a string field: path")?;
        Ok(Self { path })
    }
}

pub(crate) fn read_file_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    let input = ReadFileInput::parse(input)?;
    let resolved_path = resolve_workspace_path(workspace_root, input.path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", input.path))?;
    if !metadata.is_file() {
        return Err(format!("Path is not a file: '{}'", input.path).into());
    }
    let content = read_utf8_file_with_limit(&resolved_path, input.path, MAX_READ_FILE_BYTES)?;
    Ok(format!("Path: {}\n{}", input.path, content))
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
        let large = vec![b'a'; MAX_READ_FILE_BYTES + 1];
        fs::write(&file_path, large).unwrap();

        let output = read_file_from_workspace(workspace.path(), &json!({"path": "large.txt"}));

        assert!(output.is_err());
    }
}
