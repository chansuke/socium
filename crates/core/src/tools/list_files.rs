use crate::AgentProgress;
use crate::constants::*;
use crate::tools::error::ToolError;
use crate::tools::input::{optional_bool_field, optional_string_field};
use crate::tools::ripgrep::glob_matches;
use crate::tools::workspace::resolve_workspace_path;
use crate::tools::{Tool, ToolContext, ToolDefinition};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

pub(crate) struct ListFilesTool;

impl Tool for ListFilesTool {
    fn name(&self) -> &'static str {
        "list_files"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_files".to_string(),
            description: "List files and directories at a given path. With recursive, lists the files beneath it as ripgrep sees them: ignored files (.gitignore, .ignore) and .git are left out. Supports file glob filtering for repo navigation."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional relative path to a file or directory in the workspace. Defaults to the current directory."
                    },
                    "recursive": {
                        "type": "boolean",
                        "description": "If true, recursively lists matching files beneath the path."
                    },
                    "glob": {
                        "type": "string",
                        "description": "Optional glob filter such as '*.rs' or 'src/**/*.ts'. Applies to file paths relative to the requested path."
                    }
                },
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
        list_files_controlled(ctx.workspace_root, input, ctx.control)
    }
}

struct ListFilesInput<'a> {
    path: &'a str,
    recursive: bool,
    glob: Option<&'a str>,
}

impl<'a> ListFilesInput<'a> {
    fn parse(input: &'a Value) -> Result<Self, ToolError> {
        let input_object = input
            .as_object()
            .ok_or("list_files requires an object input")?;
        let raw_path = match input_object.get("path") {
            None | Some(Value::Null) => ".",
            Some(value) => value
                .as_str()
                .ok_or("list_files requires an optional string field: path")?,
        };
        let path = if raw_path.is_empty() { "." } else { raw_path };
        let recursive = optional_bool_field(input_object, "recursive", "list_files recursive")?
            .unwrap_or(false);
        let glob = optional_string_field(input_object, "glob", "list_files glob")?;
        Ok(Self {
            path,
            recursive,
            glob,
        })
    }
}

pub(crate) fn list_files_from_workspace(
    workspace_root: &Path,
    input: &Value,
) -> Result<String, ToolError> {
    list_files_controlled(workspace_root, input, &crate::TaskControl::default())
}
fn list_files_controlled(
    workspace_root: &Path,
    input: &Value,
    control: &crate::TaskControl,
) -> Result<String, ToolError> {
    let ListFilesInput {
        path,
        recursive,
        glob,
    } = ListFilesInput::parse(input)?;

    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect path '{}': {e}", path))?;

    if metadata.is_file() {
        if let Some(glob) = glob {
            let file_name = Path::new(path)
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("Failed to inspect file name for '{}'", path))?;
            if !glob_matches(glob, file_name) {
                return Ok(format!("Path: {path}\nGlob: {glob}\n(empty)"));
            }
        }
        return Ok(format!("Path: {}\n[file]", path));
    }

    if !metadata.is_dir() {
        return Err(format!("Path is neither a file nor directory: '{}'", path).into());
    }

    if glob.is_some_and(|g| g.len() > 512) {
        return Err("Glob exceeds 512 bytes".into());
    }
    if recursive {
        return list_recursively(&resolved_path, path, glob, control);
    }
    let entries = collect_entries(&resolved_path, glob, recursive, control)?;
    Ok(format_list_file_entries(path, glob, recursive, entries)?)
}
fn collect_entries(
    root: &Path,
    glob: Option<&str>,
    recursive: bool,
    control: &crate::TaskControl,
) -> Result<Vec<(String, String)>, String> {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut entries = Vec::new();
    let mut inspected = 0;
    let start = std::time::Instant::now();
    while let Some((directory, depth)) = stack.pop() {
        if depth > 64 {
            return Err("Directory depth exceeds 64; narrow scope".into());
        }
        for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
            inspected += 1;
            if control.is_cancelled() {
                return Err("Listing cancelled".into());
            }
            if inspected > 100000 || start.elapsed() > std::time::Duration::from_secs(5) {
                return Err("Listing scan limit reached; narrow scope".into());
            }
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if kind.is_dir() && recursive {
                stack.push((path.clone(), depth + 1));
            }
            if recursive || glob.is_some() {
                if !kind.is_file() {
                    continue;
                }
                let matched = glob.is_none_or(|g| {
                    glob_matches(g, &relative)
                        || glob_matches(g, &entry.file_name().to_string_lossy())
                });
                if matched {
                    entries.push((relative.clone(), format!("[file] {relative}")));
                }
            } else {
                let tag = if kind.is_symlink() {
                    "lnk"
                } else if kind.is_dir() {
                    "dir"
                } else {
                    "file"
                };
                let suffix = if kind.is_dir() { "/" } else { "" };
                entries.push((relative.clone(), format!("[{tag}] {relative}{suffix}")));
            }
            if entries.len() > MAX_LIST_ENTRIES {
                return Ok(entries);
            }
        }
    }
    Ok(entries)
}

/// Files beneath `resolved`, from `rg --files`, so build output and
/// dependencies that the project ignores do not crowd out its sources.
fn list_recursively(
    resolved: &Path,
    path: &str,
    glob: Option<&str>,
    control: &crate::TaskControl,
) -> Result<String, ToolError> {
    let mut command = vec![
        "rg".to_string(),
        "--no-config".into(),
        "--files".into(),
        "--hidden".into(),
        "--glob=!.git".into(),
        "--glob=!.socium".into(),
    ];
    if let Some(glob) = glob {
        command.extend(["--glob".into(), glob.into()]);
    }
    let output = crate::process::run(
        &command,
        resolved,
        std::time::Duration::from_secs(5),
        control,
    )
    .map_err(|e| format!("Listing requires ripgrep (rg); cannot execute it: {e}"))?;
    if output.cancelled {
        return Err("Listing cancelled".into());
    }
    if output.timed_out {
        return Err("Listing exceeded the 5 second deadline; narrow the path".into());
    }
    // Exit 1 is "no files"; anything else, or a diagnostic, is a failure.
    if !matches!(output.status.code(), Some(0 | 1)) || !output.stderr.is_empty() {
        return Err(format!(
            "Listing process or sandbox failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut files = text
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.strip_prefix("./").unwrap_or(line).to_string())
        .collect::<Vec<_>>();
    // A cut capture may end in a partial name.
    if output.truncated {
        files.pop();
    }
    let total = files.len();
    files.sort_unstable();
    let entries = files
        .into_iter()
        .take(MAX_LIST_ENTRIES)
        .map(|file| {
            let line = format!("[file] {file}");
            (file, line)
        })
        .collect::<Vec<_>>();
    let mut listing = format_list_file_entries(path, glob, true, entries)?;
    if total > MAX_LIST_ENTRIES || output.truncated {
        listing.push_str(&format!(
            "\n(truncated: showing {MAX_LIST_ENTRIES} of {}{} files; narrow the path or add a glob)",
            if output.truncated { "more than " } else { "" },
            total
        ));
    }
    Ok(listing)
}

fn format_list_file_entries(
    path: &str,
    glob: Option<&str>,
    recursive: bool,
    mut entries: Vec<(String, String)>,
) -> Result<String, String> {
    if entries.is_empty() {
        let mut lines = vec![format!("Path: {path}")];
        if let Some(glob) = glob {
            lines.push(format!("Glob: {glob}"));
        }
        if recursive {
            lines.push("Recursive: true".to_string());
        }
        lines.push("(empty)".to_string());
        return Ok(lines.join("\n"));
    }

    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let has_more = entries.len() > MAX_LIST_ENTRIES;
    let lines = entries
        .into_iter()
        .take(MAX_LIST_ENTRIES)
        .map(|(_, line)| line)
        .collect::<Vec<_>>();
    let mut output = vec![format!("Path: {path}")];
    if let Some(glob) = glob {
        output.push(format!("Glob: {glob}"));
    }
    if recursive {
        output.push("Recursive: true".to_string());
    }
    output.push(lines.join("\n"));
    if has_more {
        output.push(format!(
            "(truncated: showing first {MAX_LIST_ENTRIES} entries; more entries omitted)"
        ));
    }
    Ok(output.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use serde_json::json;

    #[test]
    fn a_recursive_listing_skips_git_and_ignored_build_output() {
        // Review R8: target/ took 999 of 1,000 entries and src/lib.rs was
        // not listed; without target/, most lines were under .git/.
        let workspace = make_temp_dir("workspace");
        let root = workspace.path();
        crate::test_support::init_git_repo(root);
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        for n in 0..1_500 {
            fs::write(root.join(format!("target/debug/{n}.o")), "").unwrap();
        }
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::create_dir_all(root.join(".github")).unwrap();
        fs::write(root.join(".github/ci.yml"), "").unwrap();

        let output = list_files_from_workspace(root, &json!({"recursive": true})).unwrap();
        assert!(output.contains("[file] src/lib.rs"), "{output}");
        assert!(output.contains("[file] .github/ci.yml"), "{output}");
        assert!(!output.contains("target/"), "{output}");
        assert!(!output.contains(".git/"), "{output}");

        for n in 0..1_100 {
            fs::write(root.join(format!("src/m{n}.rs")), "").unwrap();
        }
        let output =
            list_files_from_workspace(root, &json!({"recursive": true, "glob": "*.rs"})).unwrap();
        assert!(
            output.contains("(truncated: showing 1000 of 1101 files"),
            "{output}"
        );
    }

    #[test]
    fn lists_directory_from_workspace_with_default_path() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("a.txt"), "a").unwrap();
        fs::create_dir(workspace.path().join("dir")).unwrap();

        let output = list_files_from_workspace(workspace.path(), &json!({})).unwrap();

        assert!(output.contains("Path: ."));
        assert!(output.contains("[file] a.txt"));
        assert!(output.contains("[dir] dir/"));
    }

    #[test]
    fn lists_specific_subdirectory_from_workspace() {
        let workspace = make_temp_dir("workspace");
        let subdir = workspace.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("inside.txt"), "inside").unwrap();

        let output =
            list_files_from_workspace(workspace.path(), &json!({"path": "subdir"})).unwrap();

        assert!(output.contains("Path: subdir"));
        assert!(output.contains("[file] inside.txt"));
    }

    #[test]
    fn lists_file_path_from_workspace() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("one.txt"), "1").unwrap();

        let output =
            list_files_from_workspace(workspace.path(), &json!({"path": "one.txt"})).unwrap();

        assert_eq!(output, "Path: one.txt\n[file]");
    }

    #[test]
    fn lists_empty_directory_from_workspace() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir(workspace.path().join("empty")).unwrap();

        let output =
            list_files_from_workspace(workspace.path(), &json!({"path": "empty"})).unwrap();

        assert_eq!(output, "Path: empty\n(empty)");
    }

    #[test]
    fn list_files_supports_recursive_glob_filtering() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir_all(workspace.path().join("src/nested")).unwrap();
        fs::write(workspace.path().join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
        fs::write(
            workspace.path().join("src/nested/mod.rs"),
            "pub mod nested;\n",
        )
        .unwrap();
        fs::write(workspace.path().join("src/nested/readme.txt"), "note\n").unwrap();

        let output = list_files_from_workspace(
            workspace.path(),
            &json!({
                "path": "src",
                "recursive": true,
                "glob": "*.rs"
            }),
        )
        .unwrap();

        assert!(output.contains("Path: src"));
        assert!(output.contains("Glob: *.rs"));
        assert!(output.contains("Recursive: true"));
        assert!(output.contains("[file] lib.rs"));
        assert!(output.contains("[file] nested/mod.rs"));
        assert!(!output.contains("readme.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn list_files_marks_symlink_as_link() {
        use std::os::unix::fs::symlink;

        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("target.txt"), "target").unwrap();
        symlink(
            workspace.path().join("target.txt"),
            workspace.path().join("link.txt"),
        )
        .unwrap();

        let output = list_files_from_workspace(workspace.path(), &json!({})).unwrap();

        assert!(output.contains("[lnk] link.txt"));
        assert!(!output.contains("[file] link.txt"));
    }

    #[test]
    fn list_files_truncates_when_entries_exceed_limit() {
        let workspace = make_temp_dir("workspace");
        for i in 0..=MAX_LIST_ENTRIES {
            fs::write(workspace.path().join(format!("f{i:04}.txt")), "x").unwrap();
        }

        let output = list_files_from_workspace(workspace.path(), &json!({})).unwrap();
        let marker =
            format!("(truncated: showing first {MAX_LIST_ENTRIES} entries; more entries omitted)");

        assert!(output.contains(&marker));
    }
}
