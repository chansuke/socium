use crate::AgentProgress;
use crate::constants::*;
use crate::tools::error::ToolError;
use crate::tools::input::{
    optional_bounded_usize_field, optional_string_field, require_string_field,
};
use crate::tools::ripgrep::{
    SearchExecution, SearchMode, format_search_results, search_with_ripgrep,
};
use crate::tools::workspace::resolve_workspace_path;
use crate::tools::{Tool, ToolContext, ToolDefinition};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) struct SearchTextTool;

impl Tool for SearchTextTool {
    fn name(&self) -> &'static str {
        "search_text"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "search_text".to_string(),
            description: "Search repository text using ripgrep and return line-addressable matches in the form path:line:text."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Text or regex pattern to search for."
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional relative file or directory path to limit the search scope."
                    },
                    "glob": {
                        "type": "string",
                        "description": "Optional ripgrep glob filter such as '*.rs'."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": format!("Optional max number of matches to return. Must be between 1 and {MAX_SEARCH_RESULTS}.")
                    }
                },
                "required": ["pattern"],
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
        search_text_controlled(
            ctx.workspace_root,
            input,
            ctx.control,
            &ctx.settings.approval.sensitive,
        )
    }
}

struct SearchTextInput<'a> {
    pattern: &'a str,
    path: &'a str,
    glob: Option<&'a str>,
    max_results: usize,
}

impl<'a> SearchTextInput<'a> {
    fn parse(input: &'a Value) -> Result<Self, ToolError> {
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
        Ok(Self {
            pattern,
            path,
            glob,
            max_results,
        })
    }
}

#[cfg(test)]
pub(crate) fn search_text_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    search_text_controlled(
        workspace_root,
        input,
        &crate::TaskControl::default(),
        &crate::settings::Settings::default().approval.sensitive,
    )
}
fn search_text_controlled(
    workspace_root: &Path,
    input: &Value,
    control: &crate::TaskControl,
    sensitive: &crate::settings::PathPatterns,
) -> Result<String, ToolError> {
    let SearchTextInput {
        pattern,
        path,
        glob,
        max_results,
    } = SearchTextInput::parse(input)?;
    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let execution = search_with_ripgrep(
        workspace_root,
        &resolved_path,
        pattern,
        glob,
        SearchMode::SmartCase,
        control,
    )?;
    let note = execution.incomplete_note();
    let SearchExecution {
        mut matches,
        status_code,
        ..
    } = execution;
    let named = crate::approval::relative_target(workspace_root, path).unwrap_or_default();
    let mut withheld = Vec::new();
    matches.retain(|entry| {
        let keep = !crate::approval::withholds(sensitive, &entry.path, &named);
        if !keep && !withheld.contains(&entry.path) {
            withheld.push(entry.path.clone());
        }
        keep
    });
    let mut formatted = format_search_results("Pattern", pattern, path, glob, max_results, matches);
    if !withheld.is_empty() {
        formatted.push('\n');
        formatted.push_str(&crate::approval::withheld_note(&withheld));
    }

    if let Some(note) = note {
        formatted.push('\n');
        formatted.push_str(note);
    }

    match status_code {
        Some(0) | Some(1) => Ok(formatted),
        Some(code) => Err(format!("search_text failed with exit code {code}\n{formatted}").into()),
        None => Err(format!("search_text terminated unexpectedly\n{formatted}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use serde_json::json;
    use std::fs;

    #[test]
    fn a_match_in_a_minified_line_is_cut() {
        let workspace = make_temp_dir("workspace");
        let line = format!("{}needle{}", "a".repeat(5_000), "b".repeat(5_000));
        fs::write(workspace.path().join("min.js"), line).unwrap();
        let output =
            search_text_from_workspace(workspace.path(), &json!({"pattern": "needle"})).unwrap();
        assert!(output.contains("[line cut at 500 characters]"), "{output}");
        assert!(output.len() < 1_000, "{}", output.len());
    }

    #[test]
    fn a_wide_search_leaves_out_files_that_may_hold_credentials() {
        let workspace = make_temp_dir("search_secrets");
        fs::write(workspace.path().join("credentials.json"), "needle-secret\n").unwrap();
        fs::write(workspace.path().join("notes.txt"), "needle here\n").unwrap();

        let wide =
            search_text_from_workspace(workspace.path(), &json!({"pattern": "needle"})).unwrap();
        assert!(wide.contains("notes.txt:1"), "{wide}");
        assert!(!wide.contains("needle-secret"), "{wide}");
        assert!(wide.contains("credentials.json"), "{wide}");

        // Named on its own, it was approved first, so it is searched.
        let named = search_text_from_workspace(
            workspace.path(),
            &json!({"pattern": "needle", "path": "credentials.json"}),
        )
        .unwrap();
        assert!(named.contains("needle-secret"), "{named}");
    }

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
