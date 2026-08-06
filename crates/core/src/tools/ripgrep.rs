use crate::constants::*;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind};
use std::path::Path;
use std::process::Command;

pub(crate) struct SearchMatch {
    pub(crate) path: String,
    pub(crate) line_number: usize,
    pub(crate) text: String,
}

pub(crate) struct SearchExecution {
    pub(crate) matches: Vec<SearchMatch>,
    pub(crate) status_code: Option<i32>,
    pub(crate) used_ripgrep: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum SearchMode {
    SmartCase,
    FixedCaseSensitive,
}

enum SearchMatcher {
    CaseSensitive(String),
    CaseInsensitive(String),
}

impl SearchMatcher {
    fn new(pattern: &str, mode: SearchMode) -> Self {
        match mode {
            SearchMode::SmartCase if pattern.chars().any(char::is_uppercase) => {
                Self::CaseSensitive(pattern.to_string())
            }
            SearchMode::SmartCase => Self::CaseInsensitive(pattern.to_lowercase()),
            SearchMode::FixedCaseSensitive => Self::CaseSensitive(pattern.to_string()),
        }
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::CaseSensitive(pattern) => line.contains(pattern),
            Self::CaseInsensitive(pattern) => line.to_lowercase().contains(pattern),
        }
    }
}

fn parse_rg_matches(workspace_root: &Path, stdout: &[u8]) -> Result<Vec<SearchMatch>, String> {
    let text = String::from_utf8(stdout.to_vec())
        .map_err(|e| format!("ripgrep returned non UTF-8 output: {e}"))?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| parse_rg_match_line(workspace_root, line))
        .collect()
}

fn parse_rg_match_line(workspace_root: &Path, line: &str) -> Result<SearchMatch, String> {
    let mut parts = line.splitn(3, ':');
    let raw_path = parts
        .next()
        .ok_or_else(|| format!("Invalid ripgrep output line: '{line}'"))?;
    let raw_line_number = parts
        .next()
        .ok_or_else(|| format!("Invalid ripgrep output line: '{line}'"))?;
    let text = parts
        .next()
        .ok_or_else(|| format!("Invalid ripgrep output line: '{line}'"))?;
    let line_number = raw_line_number
        .parse::<usize>()
        .map_err(|e| format!("Invalid ripgrep line number '{raw_line_number}': {e}"))?;
    let path = normalize_match_path(workspace_root, raw_path);

    Ok(SearchMatch {
        path,
        line_number,
        text: text.to_string(),
    })
}

fn normalize_match_path(workspace_root: &Path, raw_path: &str) -> String {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        path.strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    } else {
        raw_path.replace('\\', "/")
    }
}

pub(crate) fn format_search_results(
    label: &str,
    query: &str,
    path: &str,
    glob: Option<&str>,
    max_results: usize,
    matches: Vec<SearchMatch>,
) -> String {
    let total = matches.len();
    let truncated = total > max_results;
    let mut lines = Vec::new();
    lines.push(format!("{label}: {query}"));
    lines.push(format!("Path: {path}"));
    if let Some(glob) = glob {
        lines.push(format!("Glob: {glob}"));
    }

    if total == 0 {
        lines.push("(no matches)".to_string());
        return lines.join("\n");
    }

    lines.extend(
        matches
            .into_iter()
            .take(max_results)
            .map(|entry| format!("{}:{}:{}", entry.path, entry.line_number, entry.text)),
    );
    if truncated {
        lines.push(format!(
            "(truncated: showing first {max_results} matches; more matches omitted)"
        ));
    }
    lines.join("\n")
}

pub(crate) fn search_with_ripgrep_fallback(
    workspace_root: &Path,
    resolved_path: &Path,
    pattern: &str,
    glob: Option<&str>,
    mode: SearchMode,
    ripgrep_program: &str,
    tool_name: &str,
) -> Result<SearchExecution, String> {
    let mut args = vec![
        "--line-number".to_string(),
        "--with-filename".to_string(),
        "--color".to_string(),
        "never".to_string(),
        "--no-heading".to_string(),
    ];
    match mode {
        SearchMode::SmartCase => args.push("--smart-case".to_string()),
        SearchMode::FixedCaseSensitive => args.push("--fixed-strings".to_string()),
    }
    if let Some(glob) = glob {
        args.push("--glob".to_string());
        args.push(glob.to_string());
    }
    args.push(pattern.to_string());
    args.push(resolved_path.to_string_lossy().into_owned());

    match Command::new(ripgrep_program)
        .args(&args)
        .current_dir(workspace_root)
        .output()
    {
        Ok(output) => Ok(SearchExecution {
            matches: parse_rg_matches(workspace_root, &output.stdout)?,
            status_code: output.status.code(),
            used_ripgrep: true,
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(SearchExecution {
            matches: fallback_search_matches(workspace_root, resolved_path, pattern, glob, mode)?,
            status_code: Some(0),
            used_ripgrep: false,
        }),
        Err(error) => Err(format!("Failed to run ripgrep for {tool_name}: {error}")),
    }
}

fn fallback_search_matches(
    workspace_root: &Path,
    resolved_path: &Path,
    pattern: &str,
    glob: Option<&str>,
    mode: SearchMode,
) -> Result<Vec<SearchMatch>, String> {
    let metadata = fs::metadata(resolved_path)
        .map_err(|e| format!("Failed to inspect path '{}': {e}", resolved_path.display()))?;
    let matcher = SearchMatcher::new(pattern, mode);
    let mut matches = Vec::new();

    if metadata.is_dir() {
        collect_fallback_search_matches_in_dir(
            workspace_root,
            resolved_path,
            glob,
            &matcher,
            &mut matches,
        )?;
    } else if metadata.is_file() {
        collect_fallback_search_matches_in_file(
            workspace_root,
            resolved_path,
            glob,
            &matcher,
            &mut matches,
        )?;
    } else {
        return Err(format!(
            "Path is neither a file nor directory: '{}'",
            resolved_path.display()
        ));
    }

    matches.sort_unstable_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.line_number.cmp(&right.line_number))
            .then(left.text.cmp(&right.text))
    });
    Ok(matches)
}

fn collect_fallback_search_matches_in_dir(
    workspace_root: &Path,
    current: &Path,
    glob: Option<&str>,
    matcher: &SearchMatcher,
    matches: &mut Vec<SearchMatch>,
) -> Result<(), String> {
    let read_dir = fs::read_dir(current)
        .map_err(|e| format!("Failed to list directory '{}': {e}", current.display()))?;
    let mut entries = Vec::new();
    for entry_result in read_dir {
        let entry = entry_result.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let sort_key = entry.file_name().to_string_lossy().to_string();
        entries.push((sort_key, entry));
    }
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));

    for (_, entry) in entries {
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to inspect directory entry type: {e}"))?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_fallback_search_matches_in_dir(workspace_root, &path, glob, matcher, matches)?;
        } else if file_type.is_file() {
            collect_fallback_search_matches_in_file(workspace_root, &path, glob, matcher, matches)?;
        }
    }

    Ok(())
}

fn collect_fallback_search_matches_in_file(
    workspace_root: &Path,
    file_path: &Path,
    glob: Option<&str>,
    matcher: &SearchMatcher,
    matches: &mut Vec<SearchMatch>,
) -> Result<(), String> {
    let display_path = normalize_match_path(workspace_root, file_path.to_string_lossy().as_ref());
    if !search_path_matches_glob(glob, &display_path) {
        return Ok(());
    }

    let metadata = fs::metadata(file_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", display_path))?;
    if metadata.len() > MAX_READ_FILE_BYTES as u64 {
        return Ok(());
    }

    let file = fs::File::open(file_path)
        .map_err(|e| format!("Failed to open file '{}': {e}", display_path))?;
    let mut reader = BufReader::new(file);
    let mut buffer = Vec::new();
    let mut line_number = 0usize;

    loop {
        buffer.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut buffer)
            .map_err(|e| format!("Failed to read file '{}': {e}", display_path))?;
        if bytes_read == 0 {
            break;
        }
        line_number += 1;

        if buffer.ends_with(b"\n") {
            buffer.pop();
        }
        if buffer.ends_with(b"\r") {
            buffer.pop();
        }

        let line = String::from_utf8_lossy(&buffer);
        if matcher.is_match(&line) {
            matches.push(SearchMatch {
                path: display_path.clone(),
                line_number,
                text: line.into_owned(),
            });
        }
    }

    Ok(())
}

fn search_path_matches_glob(glob: Option<&str>, path: &str) -> bool {
    glob.is_none_or(|pattern| {
        glob_matches(pattern, path)
            || Path::new(path)
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|file_name| glob_matches(pattern, file_name))
    })
}

pub(crate) fn glob_matches(pattern: &str, value: &str) -> bool {
    glob_match_bytes(pattern.as_bytes(), value.as_bytes())
}

fn glob_match_bytes(pattern: &[u8], value: &[u8]) -> bool {
    if pattern.is_empty() {
        return value.is_empty();
    }

    match pattern[0] {
        b'*' => {
            glob_match_bytes(&pattern[1..], value)
                || (!value.is_empty() && glob_match_bytes(pattern, &value[1..]))
        }
        b'?' => !value.is_empty() && glob_match_bytes(&pattern[1..], &value[1..]),
        byte => {
            !value.is_empty() && byte == value[0] && glob_match_bytes(&pattern[1..], &value[1..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use crate::tools::find_symbol::looks_like_rust_symbol_declaration;

    #[test]
    fn search_falls_back_when_ripgrep_is_unavailable() {
        let workspace = make_temp_dir("search-fallback");
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

        let execution = search_with_ripgrep_fallback(
            workspace.path(),
            &workspace.path().join("src"),
            "needle",
            None,
            SearchMode::SmartCase,
            "__socium_missing_rg__",
            "search_text",
        )
        .unwrap();

        assert!(!execution.used_ripgrep);
        assert_eq!(execution.status_code, Some(0));
        assert!(
            execution
                .matches
                .iter()
                .any(|entry| entry.path == "src/lib.rs" && entry.line_number == 2)
        );
        assert!(
            execution
                .matches
                .iter()
                .any(|entry| entry.path == "src/other.rs"
                    && entry.text == "const NEEDLE: &str = \"needle\";")
        );
    }

    #[test]
    fn find_symbol_falls_back_when_ripgrep_is_unavailable() {
        let workspace = make_temp_dir("find-symbol-fallback");
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

        let execution = search_with_ripgrep_fallback(
            workspace.path(),
            workspace.path(),
            "Widget",
            Some("*.rs"),
            SearchMode::FixedCaseSensitive,
            "__socium_missing_rg__",
            "find_symbol",
        )
        .unwrap();

        let SearchExecution {
            matches,
            used_ripgrep,
            ..
        } = execution;
        let matches = matches
            .into_iter()
            .filter(|entry| looks_like_rust_symbol_declaration(&entry.text, "Widget"))
            .collect::<Vec<_>>();

        assert!(!used_ripgrep);
        assert!(
            matches
                .iter()
                .any(|entry| entry.text == "pub struct Widget;")
        );
        assert!(
            matches
                .iter()
                .any(|entry| entry.text == "impl Widget { pub fn build() -> Self { Self } }")
        );
        assert!(
            !matches
                .iter()
                .any(|entry| entry.text.contains("WidgetState"))
        );
    }

    #[test]
    fn search_fallback_handles_non_utf8_files_without_failing() {
        let workspace = make_temp_dir("search-non-utf8-fallback");
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(workspace.path().join("src/good.rs"), "let needle = 1;\n").unwrap();
        fs::write(
            workspace.path().join("src/bad.rs"),
            b"let \xffneedle = 2;\n",
        )
        .unwrap();

        let execution = search_with_ripgrep_fallback(
            workspace.path(),
            &workspace.path().join("src"),
            "needle",
            Some("*.rs"),
            SearchMode::SmartCase,
            "__socium_missing_rg__",
            "search_text",
        )
        .unwrap();

        assert!(!execution.used_ripgrep);
        assert!(
            execution
                .matches
                .iter()
                .any(|entry| entry.path == "src/good.rs" && entry.text == "let needle = 1;")
        );
    }
}
