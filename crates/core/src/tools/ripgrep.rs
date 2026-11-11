use crate::constants::*;
use std::path::Path;

pub(crate) struct SearchMatch {
    pub path: String,
    pub line_number: usize,
    pub text: String,
}
pub(crate) struct SearchExecution {
    pub matches: Vec<SearchMatch>,
    pub status_code: Option<i32>,
    pub truncated: bool,
}
#[derive(Clone, Copy)]
pub(crate) enum SearchMode {
    SmartCase,
    FixedCaseSensitive,
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

pub(crate) fn search_with_ripgrep(
    workspace_root: &Path,
    resolved_path: &Path,
    pattern: &str,
    glob: Option<&str>,
    mode: SearchMode,
    control: &crate::TaskControl,
) -> Result<SearchExecution, String> {
    let mut command = vec![
        "rg".into(),
        "--no-config".into(),
        "--json".into(),
        "--color=never".into(),
        format!("--max-count={}", MAX_SEARCH_RESULTS + 1),
        "--glob=!.socium/**".into(),
    ];
    match mode {
        SearchMode::SmartCase => command.push("--smart-case".into()),
        SearchMode::FixedCaseSensitive => command.push("--fixed-strings".into()),
    }
    if let Some(glob) = glob {
        command.push("--glob".into());
        command.push(glob.into());
    }
    command.extend([
        "-e".into(),
        pattern.into(),
        "--".into(),
        resolved_path.to_string_lossy().into_owned(),
    ]);
    let output=crate::process::run(&command,workspace_root,std::time::Duration::from_secs(5),control)
        .map_err(|e|format!("Search requires ripgrep (rg); cannot execute it: {e}. No fallback with different regex/ignore semantics is used."))?;
    if output.cancelled {
        return Err("Search cancelled".into());
    }
    if output.timed_out {
        return Err("Search exceeded the 5 second deadline; narrow the scope".into());
    }
    check_search_status(&output)?;
    let matches = parse_matches(workspace_root, &output.stdout, output.truncated)?;
    Ok(SearchExecution {
        matches,
        status_code: output.status.code(),
        truncated: output.truncated,
    })
}

fn check_search_status(output: &crate::process::ProcessOutput) -> Result<(), String> {
    // Exit 1 means "no matches" only for a successfully started rg. The
    // sandbox backend also uses 1 for setup failures, with a diagnostic.
    // Never turn that failure (or incomplete output with a diagnostic) into
    // a successful empty search result.
    if !matches!(output.status.code(), Some(0 | 1)) || !output.stderr.is_empty() {
        return Err(format!(
            "Search process or sandbox failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}
fn parse_matches(root: &Path, bytes: &[u8], truncated: bool) -> Result<Vec<SearchMatch>, String> {
    let mut matches = Vec::new();
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        if truncated && !line.ends_with(b"\n") {
            break;
        }
        let event: serde_json::Value =
            serde_json::from_slice(line).map_err(|e| format!("Invalid ripgrep JSON: {e}"))?;
        if event["type"] != "match" {
            continue;
        }
        let data = &event["data"];
        let raw = data["path"]["text"]
            .as_str()
            .ok_or("Search path is not UTF-8; bytes paths are unsupported")?;
        let path = Path::new(raw)
            .strip_prefix(root)
            .unwrap_or(Path::new(raw))
            .to_string_lossy()
            .into_owned();
        let text = data["lines"]["text"]
            .as_str()
            .ok_or("Matching content is not UTF-8; binary matches are unsupported")?;
        let line_number = data["line_number"]
            .as_u64()
            .ok_or("Missing search line number")? as usize;
        matches.push(SearchMatch {
            path,
            line_number,
            text: text.trim_end_matches(['\r', '\n']).into(),
        });
        if matches.len() > MAX_SEARCH_RESULTS {
            break;
        }
    }
    Ok(matches)
}

pub(crate) fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut p, mut v) = (0, 0);
    let mut star = None;
    let mut retry = 0;
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            retry = v;
        } else if let Some(position) = star {
            p = position + 1;
            retry += 1;
            v = retry;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn backend_setup_failure_is_not_reported_as_no_matches() {
        use std::os::unix::process::ExitStatusExt;
        let mut output = crate::process::ProcessOutput {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: Vec::new(),
            timed_out: false,
            cancelled: false,
            truncated: false,
        };
        assert!(check_search_status(&output).is_ok());
        output.stderr = b"bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted".to_vec();
        assert!(check_search_status(&output).unwrap_err().contains("bwrap:"));
    }
    #[test]
    fn hyphen_pattern_colon_filename_and_ignore_rules() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a:b.txt"), "-needle\n").unwrap();
        std::fs::create_dir(root.path().join(".socium")).unwrap();
        std::fs::write(root.path().join(".socium/secret"), "-needle").unwrap();
        let result = search_with_ripgrep(
            root.path(),
            root.path(),
            "-needle",
            Some("*.txt"),
            SearchMode::SmartCase,
            &crate::TaskControl::default(),
        )
        .unwrap();
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].path, "a:b.txt");
        assert_eq!(result.matches[0].line_number, 1);
        assert!(
            search_with_ripgrep(
                root.path(),
                root.path(),
                "[",
                None,
                SearchMode::SmartCase,
                &crate::TaskControl::default()
            )
            .is_err()
        );
    }
}
