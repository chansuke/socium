use crate::AgentProgress;
use crate::constants::*;
use crate::conversation::ToolDefinition;
use crate::tool_error::ToolError;
use crate::tools::input::{
    optional_bounded_usize_field, optional_string_field, require_string_field,
};
use crate::tools::ripgrep::{
    SearchExecution, SearchMode, escape_regex, format_search_results, search_with_ripgrep,
};
use crate::tools::workspace::resolve_workspace_path;
use crate::tools::{Tool, ToolContext};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) struct FindSymbolTool;

impl Tool for FindSymbolTool {
    fn name(&self) -> &'static str {
        "find_symbol"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "find_symbol".to_string(),
            description: "Find lines that look like a Rust declaration of a name (fn, struct, enum, trait, type, mod, const, static, or an impl naming it) in .rs files only; other languages are not searched, so use search_text for them. It matches line by line, not by parsing: a declaration mentioned in a comment or a string is returned, one split across lines is missed, and same-named items in different modules all come back. It does not find references. Returns matches as path:line:text."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "The name to find, without a path (Widget, not crate::Widget)."
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
        }
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        _on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        find_symbol_controlled(
            ctx.workspace_root,
            input,
            ctx.control,
            &ctx.settings.approval.sensitive,
        )
    }
}

/// Added to an empty result, which could otherwise read as "not declared
/// anywhere" in a project written in another language.
const ONLY_RUST_NOTE: &str =
    "find_symbol reads only Rust (.rs) files; for other languages, use search_text.";

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

#[cfg(test)]
pub(crate) fn find_symbol_in_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    find_symbol_controlled(
        workspace_root,
        input,
        &crate::TaskControl::default(),
        &crate::settings::Settings::default().approval.sensitive,
    )
}
fn find_symbol_controlled(
    workspace_root: &Path,
    input: &Value,
    control: &crate::TaskControl,
    sensitive: &crate::settings::PathPatterns,
) -> Result<String, ToolError> {
    let FindSymbolInput {
        symbol,
        path,
        max_results,
    } = FindSymbolInput::parse(input)?;
    let resolved_path = resolve_workspace_path(workspace_root, path)?;

    // Search for declaration-like lines only. Searching for the bare name
    // would spend ripgrep's per-file match limit on references and could
    // stop before a declaration further down the file.
    let execution = search_with_ripgrep(
        workspace_root,
        &resolved_path,
        &declaration_pattern(symbol),
        Some("*.rs"),
        SearchMode::Regex,
        control,
    )?;
    let note = execution.incomplete_note();
    let SearchExecution {
        matches,
        status_code,
        ..
    } = execution;
    let named = crate::approval::relative_target(workspace_root, path).unwrap_or_default();
    let mut withheld = Vec::new();
    let matches = matches
        .into_iter()
        .filter(|entry| looks_like_rust_symbol_declaration(&entry.text, symbol))
        .filter(|entry| {
            let keep = !crate::approval::withholds(sensitive, &entry.path, &named);
            if !keep && !withheld.contains(&entry.path) {
                withheld.push(entry.path.clone());
            }
            keep
        })
        .collect::<Vec<_>>();
    let found = !matches.is_empty();
    let mut formatted =
        format_search_results("Symbol", symbol, path, Some("*.rs"), max_results, matches);
    if !found {
        formatted.push('\n');
        formatted.push_str(ONLY_RUST_NOTE);
    }
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
        Some(code) => Err(format!("find_symbol failed with exit code {code}\n{formatted}").into()),
        None => Err(format!("find_symbol terminated unexpectedly\n{formatted}").into()),
    }
}

/// A ripgrep pattern for every line `looks_like_rust_symbol_declaration`
/// can accept, so that filter sees all candidates.
fn declaration_pattern(symbol: &str) -> String {
    let symbol = escape_regex(symbol);
    format!("(?:fn|struct|enum|trait|type|mod|const|static) {symbol}|^\\s*impl .*{symbol}")
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
    fn a_wide_symbol_search_leaves_out_files_that_may_hold_credentials() {
        let workspace = make_temp_dir("symbol_secrets");
        fs::write(
            workspace.path().join(".env.rs"),
            "const API_KEY: &str = \"sk-secret\";\n",
        )
        .unwrap();
        fs::write(
            workspace.path().join("lib.rs"),
            "const API_KEY: &str = \"\";\n",
        )
        .unwrap();
        let output =
            find_symbol_in_workspace(workspace.path(), &json!({"symbol": "API_KEY", "path": "."}))
                .unwrap();
        assert!(output.contains("lib.rs:1"), "{output}");
        assert!(!output.contains("sk-secret"), "{output}");
        assert!(output.contains(".env.rs"), "{output}");
    }

    /// What a line-by-line heuristic cannot tell apart, kept as it is so
    /// a change to it is a decision: a declaration named in a comment or a
    /// string counts, two `build`s in different modules both come back with
    /// nothing to choose between them, and a declaration split over lines
    /// is missed.
    #[test]
    fn the_limits_of_matching_declarations_line_by_line() {
        let workspace = make_temp_dir("symbol_limits");
        fs::write(
            workspace.path().join("lib.rs"),
            "// fn build was removed; see make\n\
             const HELP: &str = \"fn build is gone\";\n\
             pub fn\n\
             \x20   build() {}\n\
             mod a { pub fn build() {} }\n\
             mod b { pub fn build() {} }\n",
        )
        .unwrap();

        let output =
            find_symbol_in_workspace(workspace.path(), &json!({"symbol": "build"})).unwrap();

        for line in ["lib.rs:1:", "lib.rs:2:", "lib.rs:5:", "lib.rs:6:"] {
            assert!(output.contains(line), "{line} missing: {output}");
        }
        for line in ["lib.rs:3:", "lib.rs:4:"] {
            assert!(!output.contains(line), "{line} found: {output}");
        }
    }

    #[test]
    fn other_languages_are_not_searched_and_an_empty_result_says_so() {
        let workspace = make_temp_dir("symbol_languages");
        fs::write(
            workspace.path().join("widget.py"),
            "class Widget:\n    pass\n",
        )
        .unwrap();
        fs::write(
            workspace.path().join("widget.ts"),
            "export class Widget {}\n",
        )
        .unwrap();

        let output =
            find_symbol_in_workspace(workspace.path(), &json!({"symbol": "Widget"})).unwrap();

        assert!(output.contains("(no matches)"), "{output}");
        assert!(
            output.contains("only Rust (.rs) files") && output.contains("search_text"),
            "{output}"
        );
    }

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

    #[test]
    fn a_declaration_after_more_than_201_references_is_found() {
        let workspace = make_temp_dir("symbol-late-declaration");
        let mut source = String::from("use crate::Widget;\n");
        for line in 0..250 {
            source.push_str(&format!("let w{line}: Widget = Widget::default();\n"));
        }
        source.push_str(
            "pub struct Widget;\nimpl Default for Widget { fn default() -> Self { Widget } }\n",
        );
        fs::write(workspace.path().join("late.rs"), source).unwrap();

        let output =
            find_symbol_in_workspace(workspace.path(), &json!({"symbol": "Widget"})).unwrap();

        assert!(
            output.contains("late.rs:252:pub struct Widget;"),
            "{output}"
        );
        assert!(
            output.contains("late.rs:253:impl Default for Widget"),
            "{output}"
        );
        assert!(!output.contains("(no matches)"));
        assert!(!output.contains("incomplete"), "{output}");
    }

    #[test]
    fn a_search_cut_off_at_its_match_limit_says_it_is_incomplete() {
        let workspace = make_temp_dir("symbol-incomplete");
        let mut source = String::new();
        for line in 0..(MAX_SEARCH_RESULTS + 20) {
            source.push_str(&format!("impl Widget {{ fn f{line}() {{}} }}\n"));
        }
        source.push_str("pub struct Widget;\n");
        fs::write(workspace.path().join("many.rs"), source).unwrap();

        let output =
            find_symbol_in_workspace(workspace.path(), &json!({"symbol": "Widget"})).unwrap();

        assert!(
            output.contains("(incomplete: the search stopped at its match limit"),
            "{output}"
        );
    }

    #[test]
    fn symbols_with_regex_characters_are_matched_literally() {
        assert_eq!(
            declaration_pattern("a.b"),
            "(?:fn|struct|enum|trait|type|mod|const|static) a\\.b|^\\s*impl .*a\\.b"
        );
        let workspace = make_temp_dir("symbol-literal");
        fs::write(
            workspace.path().join("x.rs"),
            "fn r#match() {}\nfn aXb() {}\n",
        )
        .unwrap();
        let output = find_symbol_in_workspace(workspace.path(), &json!({"symbol": "a.b"})).unwrap();
        assert!(output.contains("(no matches)"), "{output}");
    }
}
