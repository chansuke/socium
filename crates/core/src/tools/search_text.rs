use crate::constants::*;
use crate::tools::input::{
    optional_bounded_usize_field, optional_string_field, require_string_field,
};
use crate::tools::ripgrep::{
    SearchExecution, SearchMode, format_search_results, search_with_ripgrep_fallback,
};
use crate::tools::workspace::resolve_workspace_path;
use serde_json::Value;
use std::path::Path;

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
    let execution = search_with_ripgrep_fallback(
        workspace_root,
        &resolved_path,
        pattern,
        glob,
        SearchMode::SmartCase,
        "rg",
        "search_text",
    )?;
    let SearchExecution {
        matches,
        status_code,
        used_ripgrep,
    } = execution;
    let formatted = format_search_results("Pattern", pattern, path, glob, max_results, matches);

    if !used_ripgrep {
        return Ok(formatted);
    }

    match status_code {
        Some(0) | Some(1) => Ok(formatted),
        Some(code) => Err(format!(
            "search_text failed with exit code {code}\n{formatted}"
        )),
        None => Err(format!("search_text terminated unexpectedly\n{formatted}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use serde_json::json;
    use std::fs;

    #[test]
    fn search_text_returns_line_addressable_matches() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(
            workspace.path().join("src/lib.rs"),
            "pub fn alpha() {}\nlet needle = 1;\n",
        )
        .unwrap();
        fs::write(
            workspace.path().join("src/other.rs"),
            "const NEEDLE: &str = \"needle\";\n",
        )
        .unwrap();

        let output = search_text_from_workspace(
            workspace.path(),
            &json!({
                "pattern": "needle",
                "path": "src"
            }),
        )
        .unwrap();

        assert!(output.contains("Pattern: needle"));
        assert!(output.contains("Path: src"));
        assert!(output.contains("src/lib.rs:2:let needle = 1;"));
        assert!(output.contains("src/other.rs:1:const NEEDLE: &str = \"needle\";"));
    }

    #[test]
    fn search_text_respects_glob_and_max_results() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(workspace.path().join("src/a.rs"), "needle\n").unwrap();
        fs::write(workspace.path().join("src/b.rs"), "needle\n").unwrap();
        fs::write(workspace.path().join("src/c.txt"), "needle\n").unwrap();

        let output = search_text_from_workspace(
            workspace.path(),
            &json!({
                "pattern": "needle",
                "glob": "*.rs",
                "max_results": 1
            }),
        )
        .unwrap();

        assert!(output.contains("Glob: *.rs"));
        assert!(output.contains("src/a.rs:1:needle") || output.contains("src/b.rs:1:needle"));
        assert!(!output.contains("src/c.txt"));
        assert!(output.contains("(truncated: showing first 1 matches; more matches omitted)"));
    }
}
