use crate::AgentProgress;
use crate::constants::*;
use crate::tools::error::ToolError;
use crate::tools::input::{
    optional_bounded_usize_field, optional_string_field, require_string_field,
};
use crate::tools::ripgrep::{
    SearchExecution, SearchMode, format_search_results, search_with_ripgrep_fallback,
};
use crate::tools::workspace::resolve_workspace_path;
use crate::tools::{CustomToolDefinition, Tool, ToolContext, ToolDefinition};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) struct FindSymbolTool;

impl Tool for FindSymbolTool {
    fn name(&self) -> &'static str {
        "find_symbol"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::Custom(CustomToolDefinition {
            name: "find_symbol".to_string(),
            description: "Find declaration-like Rust symbol references for quick navigation. Returns line-addressable matches in the form path:line:text."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Rust symbol name to search for."
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional relative file or directory path to limit the search scope."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": format!("Optional max number of matches to return. Must be between 1 and {MAX_SEARCH_RESULTS}.")
                    }
                },
                "required": ["symbol"],
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
        find_symbol_in_workspace(ctx.workspace_root, input)
    }
}

struct FindSymbolInput<'a> {
    symbol: &'a str,
    path: &'a str,
    max_results: usize,
}

impl<'a> FindSymbolInput<'a> {
    fn parse(input: &'a Value) -> Result<Self, ToolError> {
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
        Ok(Self {
            symbol,
            path,
            max_results,
        })
    }
}

pub(crate) fn find_symbol_in_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    let FindSymbolInput {
        symbol,
        path,
        max_results,
    } = FindSymbolInput::parse(input)?;
    let resolved_path = resolve_workspace_path(workspace_root, path)?;

    let execution = search_with_ripgrep_fallback(
        workspace_root,
        &resolved_path,
        symbol,
        Some("*.rs"),
        SearchMode::FixedCaseSensitive,
        "rg",
        "find_symbol",
    )?;
    let SearchExecution {
        matches,
        status_code,
        used_ripgrep,
    } = execution;
    let matches = matches
        .into_iter()
        .filter(|entry| looks_like_rust_symbol_declaration(&entry.text, symbol))
        .collect::<Vec<_>>();
    let formatted =
        format_search_results("Symbol", symbol, path, Some("*.rs"), max_results, matches);

    if !used_ripgrep {
        return Ok(formatted);
    }

    match status_code {
        Some(0) | Some(1) => Ok(formatted),
        Some(code) => Err(format!("find_symbol failed with exit code {code}\n{formatted}").into()),
        None => Err(format!("find_symbol terminated unexpectedly\n{formatted}").into()),
    }
}

pub(crate) fn looks_like_rust_symbol_declaration(line: &str, symbol: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use serde_json::json;
    use std::fs;

    #[test]
    fn find_symbol_returns_rust_declaration_matches() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(
            workspace.path().join("src/lib.rs"),
            "pub struct Widget;\nfn helper() {}\nimpl Widget { pub fn build() -> Self { Self } }\n",
        )
        .unwrap();
        fs::write(
            workspace.path().join("src/other.rs"),
            "pub enum WidgetState { Ready }\n",
        )
        .unwrap();

        let output = find_symbol_in_workspace(
            workspace.path(),
            &json!({
                "symbol": "Widget"
            }),
        )
        .unwrap();

        assert!(output.contains("Symbol: Widget"));
        assert!(output.contains("src/lib.rs:1:pub struct Widget;"));
        assert!(output.contains("src/lib.rs:3:impl Widget { pub fn build() -> Self { Self } }"));
        assert!(!output.contains("WidgetState"));
    }
}
