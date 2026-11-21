use crate::constants::*;
use crate::conversation::ToolDefinition;
use crate::progress::AgentProgress;
use crate::tool_error::ToolError;
use crate::tools::git::maybe_prepend_dirty_worktree_warning;
use crate::tools::input::{optional_bool_field, require_string_field};
use crate::tools::text_file::{read_utf8_file_and_mode, read_utf8_file_with_limit};
use crate::tools::{Tool, ToolContext};
use crate::workspace::{
    resolve_workspace_mutation_path, resolve_workspace_path, resolve_workspace_path_for_create,
};
use serde_json::{Value, json};
#[cfg(test)]
use std::fs;
use std::path::Path;

pub(crate) struct ApplyPatchTool;

impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "apply_patch".to_string(),
            description: "Apply a structured patch with add, delete, or multi-hunk update operations inside the workspace. The result shows the unified diff of what changed."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dry_run": {
                        "type": "boolean",
                        "description": "If true, validates the patch and returns the unified diff it would make, without writing files."
                    },
                    "patch": {
                        "type": "string",
                        "description": "Patch text using *** Begin Patch / *** End Patch blocks with Add File, Delete File, or Update File sections. In Update File, @@ starts a hunk; lines start with ' ' (context), '-', or '+'; *** End of File anchors the hunk at the end of the file."
                    }
                },
                "required": ["patch"],
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
        apply_patch_from_workspace(ctx.workspace_root, input, ctx.edit_origin)
    }
}

enum PatchAction {
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<PatchHunk>,
    },
}

struct PatchHunk {
    lines: Vec<HunkLine>,
    /// Marked `*** End of File`: the hunk matches the file's last lines.
    at_end: bool,
}

enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

impl PatchHunk {
    fn old_lines(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter_map(|line| match line {
                HunkLine::Context(text) | HunkLine::Remove(text) => Some(text.as_str()),
                HunkLine::Add(_) => None,
            })
            .collect()
    }
}

struct ApplyPatchInput<'a> {
    patch: &'a str,
    dry_run: bool,
}

impl<'a> ApplyPatchInput<'a> {
    fn parse(input: &'a Value) -> Result<Self, ToolError> {
        let input_object = input
            .as_object()
            .ok_or("apply_patch requires an object input")?;
        let patch = require_string_field(input_object, "patch", "apply_patch")?;
        let dry_run =
            optional_bool_field(input_object, "dry_run", "apply_patch dry_run")?.unwrap_or(false);
        Ok(Self { patch, dry_run })
    }
}

pub(crate) fn apply_patch_from_workspace(
    workspace_root: &Path,
    input: &Value,
    edit_origin: Option<&crate::tools::edit_history::EditOrigin>,
) -> Result<String, ToolError> {
    let ApplyPatchInput { patch, dry_run } = ApplyPatchInput::parse(input)?;
    let actions = parse_patch_actions(patch)?;
    let plan = plan_patch(workspace_root, &actions)?;
    let diff = crate::tools::edit_history::describe(&plan);
    if dry_run {
        return Ok(format!("Preview: true\n{diff}"));
    }
    let report = actions
        .iter()
        .map(|a| match a {
            PatchAction::Add { path, .. } => format!("Added file: {path}"),
            PatchAction::Delete { path } => format!("Deleted file: {path}"),
            PatchAction::Update {
                path,
                move_to: Some(to),
                ..
            } => format!("Moved file: {path} -> {to}\nUpdated file: {to}"),
            PatchAction::Update { path, .. } => format!("Updated file: {path}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    crate::tools::edit_history::apply_changes(workspace_root, plan, edit_origin)?;
    Ok(maybe_prepend_dirty_worktree_warning(
        workspace_root,
        format!("{report}\n{diff}"),
    ))
}

/// The diff a patch would make, for its approval request.
pub(crate) fn preview(workspace_root: &Path, input: &Value) -> Result<String, ToolError> {
    let ApplyPatchInput { patch, .. } = ApplyPatchInput::parse(input)?;
    let plan = plan_patch(workspace_root, &parse_patch_actions(patch)?)?;
    Ok(crate::tools::edit_history::describe(&plan))
}

fn plan_patch(
    root: &Path,
    actions: &[PatchAction],
) -> Result<Vec<crate::tools::edit_history::Mutation>, ToolError> {
    use crate::tools::edit_history::Mutation;
    let mut plan = Vec::new();
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    for action in actions {
        match action {
            PatchAction::Add { path, content } => {
                let target = resolve_workspace_path_for_create(root, path)?;
                if target.exists() {
                    return Err(format!("Cannot add file '{path}': path already exists").into());
                }
                plan.push(Mutation {
                    path: path.clone(),
                    before: None,
                    after: Some(content.clone()),
                    ..Mutation::default()
                });
            }
            PatchAction::Delete { path } => {
                let target = resolve_workspace_path(root, path)?;
                let content = read_utf8_file_with_limit(&target, path, MAX_READ_FILE_BYTES)?;
                plan.push(Mutation {
                    path: path.clone(),
                    before: Some(content),
                    after: None,
                    ..Mutation::default()
                });
            }
            PatchAction::Update {
                path,
                move_to,
                hunks,
            } => {
                let source = resolve_workspace_path(root, path)?;
                let (content, mode) = read_utf8_file_and_mode(&source, path, MAX_READ_FILE_BYTES)?;
                let updated = apply_patch_hunks_to_text(&content, path, hunks)?;
                if let Some(destination) = move_to {
                    let target = resolve_workspace_path_for_create(root, destination)?;
                    if target == source {
                        return Err("Move source and destination are the same file".into());
                    }
                    if target.exists() {
                        return Err(
                            format!("Cannot move '{path}': destination already exists").into()
                        );
                    }
                    // The moved file keeps the source's mode, such as its
                    // executable bit.
                    plan.push(Mutation {
                        path: path.clone(),
                        before: Some(content),
                        after: None,
                        before_mode: mode,
                        after_mode: None,
                    });
                    plan.push(Mutation {
                        path: destination.clone(),
                        before: None,
                        after: Some(updated),
                        before_mode: None,
                        after_mode: mode,
                    });
                } else {
                    plan.push(Mutation {
                        path: path.clone(),
                        before: Some(content),
                        after: Some(updated),
                        before_mode: mode,
                        after_mode: mode,
                    });
                }
            }
        }
    }
    for change in &plan {
        if change
            .after
            .as_ref()
            .is_some_and(|s| s.len() > MAX_READ_FILE_BYTES)
        {
            return Err("Patched file exceeds supported file size limit".into());
        }
        let path = resolve_workspace_mutation_path(root, &change.path)?;
        if seen
            .iter()
            .any(|p| p.starts_with(&path) || path.starts_with(p))
        {
            return Err("Overlapping patch paths; split the patch".into());
        }
        seen.push(path);
    }
    Ok(plan)
}

/// Every path a patch names, move destinations included, before any of
/// them is resolved.
pub(crate) fn patch_paths(patch: &str) -> Result<Vec<String>, String> {
    Ok(parse_patch_actions(patch)?
        .into_iter()
        .flat_map(|action| match action {
            PatchAction::Add { path, .. } | PatchAction::Delete { path } => vec![path],
            PatchAction::Update { path, move_to, .. } => {
                std::iter::once(path).chain(move_to).collect()
            }
        })
        .collect())
}

fn parse_patch_actions(patch: &str) -> Result<Vec<PatchAction>, String> {
    let lines = patch.lines().collect::<Vec<_>>();
    if lines.first().copied() != Some("*** Begin Patch") {
        return Err("apply_patch patch must start with '*** Begin Patch'".to_string());
    }
    if lines.last().copied() != Some("*** End Patch") {
        return Err("apply_patch patch must end with '*** End Patch'".to_string());
    }

    let mut actions = Vec::new();
    let mut index = 1usize;
    while index < lines.len() - 1 {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut content = String::new();
            while index < lines.len() - 1 && !lines[index].starts_with("*** ") {
                let patch_line = lines[index];
                let added = patch_line
                    .strip_prefix('+')
                    .ok_or_else(|| format!("Add File section for '{}' requires '+' lines", path))?;
                // Every added line ends with a newline, the last one too.
                content.push_str(added);
                content.push('\n');
                index += 1;
            }
            if lines[index] == END_OF_FILE {
                index += 1;
            }
            actions.push(PatchAction::Add {
                path: path.to_string(),
                content,
            });
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            actions.push(PatchAction::Delete {
                path: path.to_string(),
            });
            index += 1;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let move_to = if index < lines.len() - 1 {
                lines[index]
                    .strip_prefix("*** Move to: ")
                    .map(ToString::to_string)
            } else {
                None
            };
            if move_to.is_some() {
                index += 1;
            }
            let mut body = Vec::new();
            while index < lines.len() - 1
                && (!lines[index].starts_with("*** ") || lines[index] == END_OF_FILE)
            {
                body.push(lines[index]);
                index += 1;
            }
            let hunks = parse_patch_hunks(path, &body)?;
            actions.push(PatchAction::Update {
                path: path.to_string(),
                move_to,
                hunks,
            });
            continue;
        }
        return Err(format!("Unsupported patch header: {line}"));
    }

    if actions.is_empty() {
        return Err("apply_patch patch did not contain any file actions".to_string());
    }

    Ok(actions)
}

const END_OF_FILE: &str = "*** End of File";

fn parse_patch_hunks(path: &str, lines: &[&str]) -> Result<Vec<PatchHunk>, String> {
    let mut hunks = Vec::new();
    let mut current = Vec::new();

    for line in lines {
        if line.starts_with("@@") {
            if !current.is_empty() {
                hunks.push(PatchHunk {
                    lines: std::mem::take(&mut current),
                    at_end: false,
                });
            }
            continue;
        }
        if *line == END_OF_FILE {
            if current.is_empty() {
                return Err(format!("'{END_OF_FILE}' must follow a hunk in '{path}'"));
            }
            hunks.push(PatchHunk {
                lines: std::mem::take(&mut current),
                at_end: true,
            });
            continue;
        }
        // An empty line is a blank context line whose leading space was
        // trimmed, as editors and models often do.
        let (prefix, content) = line
            .chars()
            .next()
            .map(|prefix| (prefix, &line[prefix.len_utf8()..]))
            .unwrap_or((' ', ""));
        if content.contains('\r') {
            return Err(format!(
                "Patch line for '{}' contains a carriage return; line endings are kept from the file",
                path
            ));
        }
        current.push(match prefix {
            ' ' => HunkLine::Context(content.to_string()),
            '-' => HunkLine::Remove(content.to_string()),
            '+' => HunkLine::Add(content.to_string()),
            _ => {
                return Err(format!(
                    "Invalid patch line prefix '{}' in update for '{}'",
                    prefix, path
                ));
            }
        });
    }

    if !current.is_empty() {
        hunks.push(PatchHunk {
            lines: current,
            at_end: false,
        });
    }
    if hunks.is_empty() {
        return Err(format!(
            "Update File section for '{}' did not contain any hunks",
            path
        ));
    }

    Ok(hunks)
}

/// One line of a file with the terminator it was read with: "\n", "\r\n",
/// or "" for a last line without one.
struct FileLine {
    text: String,
    ending: &'static str,
}

/// Applies hunks line by line. Every line the patch does not touch keeps
/// its own terminator, so a file mixing LF and CRLF changes only where the
/// patch changes it. Added lines take the terminator of the line they
/// replace or precede. Whether the file ends with a newline is kept.
fn apply_patch_hunks_to_text(
    content: &str,
    path: &str,
    hunks: &[PatchHunk],
) -> Result<String, String> {
    let mut lines = split_file_lines(content, path)?;
    let trailing_newline = lines.last().is_none_or(|line| !line.ending.is_empty());
    let default_ending = dominant_ending(&lines);
    let mut search_start = 0usize;

    for hunk in hunks {
        let old_lines = hunk.old_lines();
        if old_lines.is_empty() {
            return Err(format!(
                "apply_patch does not support insertion-only hunks yet for '{}'",
                path
            ));
        }
        let texts = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>();
        let position = if hunk.at_end {
            let start = texts.len().checked_sub(old_lines.len());
            start
                .filter(|start| *start >= search_start && texts[*start..] == old_lines[..])
                .ok_or_else(|| format!("Failed to match patch hunk at the end of '{}'", path))?
        } else {
            find_hunk_position(&texts, &old_lines, search_start)
                .ok_or_else(|| format!("Failed to match patch hunk in '{}'", path))?
        };
        let old = lines
            .splice(position..position + old_lines.len(), [])
            .collect::<Vec<_>>();
        let mut old = old.into_iter().peekable();
        let mut replacement = Vec::new();
        let mut last_ending = position
            .checked_sub(1)
            .map_or(default_ending, |index| lines[index].ending);
        for line in &hunk.lines {
            match line {
                HunkLine::Context(_) => {
                    let kept = old.next().expect("matched context line");
                    last_ending = kept.ending;
                    replacement.push(kept);
                }
                HunkLine::Remove(_) => {
                    last_ending = old.next().expect("matched removed line").ending;
                }
                HunkLine::Add(text) => {
                    let ending = old.peek().map_or(last_ending, |next| next.ending);
                    replacement.push(FileLine {
                        text: text.clone(),
                        ending,
                    });
                }
            }
        }
        lines.splice(position..position, replacement);
        search_start = position;
    }

    let count = lines.len();
    let mut updated = String::with_capacity(content.len());
    for (index, line) in lines.iter().enumerate() {
        updated.push_str(&line.text);
        let last = index + 1 == count;
        updated.push_str(if last && !trailing_newline {
            ""
        } else if line.ending.is_empty() {
            default_ending
        } else {
            line.ending
        });
    }
    Ok(updated)
}

/// Splits `content` into lines with their terminators. A carriage return
/// outside a CRLF pair has no safe line-based reading, so the file is
/// refused before anything is planned or written.
fn split_file_lines(content: &str, path: &str) -> Result<Vec<FileLine>, String> {
    let mut lines = Vec::new();
    for piece in content.split_inclusive('\n') {
        let (text, ending) = if let Some(text) = piece.strip_suffix("\r\n") {
            (text, "\r\n")
        } else if let Some(text) = piece.strip_suffix('\n') {
            (text, "\n")
        } else {
            (piece, "")
        };
        if text.contains('\r') {
            return Err(format!(
                "Cannot patch '{}': it contains a carriage return that does not end a line; edit it another way",
                path
            ));
        }
        lines.push(FileLine {
            text: text.to_string(),
            ending,
        });
    }
    Ok(lines)
}

/// The terminator most lines use, for lines that need one and have no
/// neighbour to copy it from. LF when there is a tie or no terminator.
fn dominant_ending(lines: &[FileLine]) -> &'static str {
    let crlf = lines.iter().filter(|line| line.ending == "\r\n").count();
    let lf = lines.iter().filter(|line| line.ending == "\n").count();
    if crlf > lf { "\r\n" } else { "\n" }
}

fn find_hunk_position(lines: &[&str], needle: &[&str], search_start: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > lines.len() || search_start > lines.len() {
        return None;
    }

    let mut matches = (search_start..=lines.len().saturating_sub(needle.len()))
        .filter(|start| lines[*start..*start + needle.len()] == *needle);
    let first = matches.next()?;
    if matches.next().is_some() {
        None
    } else {
        Some(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::ContentBlock;
    use crate::test_support::{init_git_repo, make_temp_dir};
    use crate::tools::run_tool_call;
    use serde_json::json;

    #[test]
    fn invalid_later_action_and_dry_run_have_no_side_effects() {
        let workspace = make_temp_dir("patch-atomic-validation");
        let patch = "*** Begin Patch\n*** Add File: first\n+hello\n*** Update File: missing\n@@\n-old\n+new\n*** End Patch";
        for dry_run in [true, false] {
            assert!(
                apply_patch_from_workspace(
                    workspace.path(),
                    &json!({"patch":patch,"dry_run":dry_run}),
                    None
                )
                .is_err()
            );
        }
        assert!(!workspace.path().join("first").exists());
        assert!(!workspace.path().join(".socium").exists());
    }
    #[cfg(unix)]
    #[test]
    fn patches_naming_a_fifo_are_refused_without_blocking() {
        let workspace = make_temp_dir("patch-fifo");
        crate::test_support::make_fifo(&workspace.path().join("pipe"));
        let started = std::time::Instant::now();
        for patch in [
            "*** Begin Patch\n*** Update File: pipe\n@@\n-old\n+new\n*** End Patch",
            "*** Begin Patch\n*** Delete File: pipe\n*** End Patch",
            "*** Begin Patch\n*** Update File: pipe\n*** Move to: moved\n@@\n-old\n+new\n*** End Patch",
        ] {
            for dry_run in [true, false] {
                let err = apply_patch_from_workspace(
                    workspace.path(),
                    &json!({"patch": patch, "dry_run": dry_run}),
                    None,
                )
                .unwrap_err();
                assert!(err.to_string().contains("Not a regular file"), "{err}");
            }
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(!workspace.path().join("moved").exists());
        assert!(!workspace.path().join(".socium").exists());
    }

    /// Applies `patch` to a file holding `before`, first as a dry run and
    /// then for real, and returns the written bytes. The preview must
    /// show exactly the bytes that get written.
    fn patch_bytes(before: &str, patch: &str) -> Result<String, String> {
        let workspace = make_temp_dir("patch-eol");
        let file = workspace.path().join("f.txt");
        fs::write(&file, before).unwrap();
        let input = |dry_run| json!({"patch": patch, "dry_run": dry_run});
        let preview = apply_patch_from_workspace(workspace.path(), &input(true), None)
            .map_err(|e| e.to_string())?;
        assert_eq!(fs::read_to_string(&file).unwrap(), before, "dry run wrote");
        apply_patch_from_workspace(workspace.path(), &input(false), None)
            .map_err(|e| e.to_string())?;
        let after = fs::read_to_string(&file).unwrap();
        assert_eq!(
            apply_unified(before, &preview),
            after,
            "preview and write differ"
        );
        Ok(after)
    }

    /// Applies the unified diff in a preview to `before`, byte for byte.
    fn apply_unified(before: &str, preview: &str) -> String {
        let old = before.split_inclusive('\n').collect::<Vec<_>>();
        let lines = preview.split_inclusive('\n').collect::<Vec<_>>();
        let mut out = String::new();
        let mut at = 0;
        let mut index = lines.iter().position(|l| l.starts_with("@@")).unwrap();
        while index < lines.len() {
            let line = lines[index];
            if let Some(header) = line.strip_prefix("@@ -") {
                let start = header
                    .split([',', ' '])
                    .next()
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                let empty = header.split(' ').next().unwrap().ends_with(",0");
                let start = if empty { start } else { start - 1 };
                while at < start {
                    out.push_str(old[at]);
                    at += 1;
                }
            } else if let Some(text) = line.strip_prefix('+') {
                // Only the "\ No newline" note leaves a line unterminated.
                let text = text.strip_suffix('\n').unwrap_or(text);
                out.push_str(text);
                if !lines.get(index + 1).is_some_and(|n| n.starts_with('\\')) {
                    out.push('\n');
                }
            } else if line.starts_with(' ') {
                out.push_str(old[at]);
                at += 1;
            } else if line.starts_with('-') {
                at += 1;
            }
            index += 1;
        }
        for line in &old[at..] {
            out.push_str(line);
        }
        out
    }

    #[test]
    fn untouched_lines_keep_their_own_line_endings() {
        let patch = "*** Begin Patch\n*** Update File: f.txt\n@@\n b\n-c\n+C\n+C2\n*** End Patch";
        for (before, expected) in [
            ("a\nb\nc\nd\n", "a\nb\nC\nC2\nd\n"),
            ("a\r\nb\r\nc\r\nd\r\n", "a\r\nb\r\nC\r\nC2\r\nd\r\n"),
            // Mixed: only the replaced line's ending is reused.
            ("a\r\nb\nc\r\nd\n", "a\r\nb\nC\r\nC2\r\nd\n"),
            ("a\nb\r\nc\nd\r\n", "a\nb\r\nC\nC2\nd\r\n"),
            // No newline at the end stays that way.
            ("a\nb\nc", "a\nb\nC\nC2"),
            ("a\r\nb\r\nc", "a\r\nb\r\nC\r\nC2"),
        ] {
            assert_eq!(patch_bytes(before, patch).unwrap(), expected, "{before:?}");
        }
    }

    #[test]
    fn lines_added_after_an_unterminated_last_line_get_an_ending() {
        let patch = "*** Begin Patch\n*** Update File: f.txt\n@@\n-b\n+b\n+c\n*** End Patch";
        assert_eq!(patch_bytes("a\r\nb", patch).unwrap(), "a\r\nb\r\nc");
        assert_eq!(patch_bytes("a\nb\n", patch).unwrap(), "a\nb\nc\n");
    }

    #[test]
    fn a_bare_carriage_return_is_refused_before_any_write() {
        let patch = "*** Begin Patch\n*** Update File: f.txt\n@@\n-b\n+B\n*** End Patch";
        for before in ["a\rb\n", "a\nb\rc\n", "b\n\r"] {
            let workspace = make_temp_dir("patch-cr");
            let file = workspace.path().join("f.txt");
            fs::write(&file, before).unwrap();
            for dry_run in [true, false] {
                let err = apply_patch_from_workspace(
                    workspace.path(),
                    &json!({"patch": patch, "dry_run": dry_run}),
                    None,
                )
                .unwrap_err();
                assert!(err.to_string().contains("carriage return"), "{err}");
            }
            assert_eq!(fs::read_to_string(&file).unwrap(), before);
            assert!(!workspace.path().join(".socium").exists());
        }
    }

    #[test]
    fn ambiguous_hunks_are_refused_and_move_undo_is_grouped() {
        let workspace = make_temp_dir("patch-move");
        fs::write(workspace.path().join("f"), "same\nsame\n").unwrap();
        assert!(apply_patch_from_workspace(workspace.path(),&json!({"patch":"*** Begin Patch\n*** Update File: f\n@@\n-same\n+new\n*** End Patch"}), None).is_err());
        fs::write(workspace.path().join("f"), "old\r\n").unwrap();
        apply_patch_from_workspace(workspace.path(),&json!({"patch":"*** Begin Patch\n*** Update File: f\n*** Move to: g\n@@\n-old\n+new\n*** End Patch"}), None).unwrap();
        assert_eq!(
            fs::read_to_string(workspace.path().join("g")).unwrap(),
            "new\r\n"
        );
        crate::tools::edit_history::undo(workspace.path(), "g").unwrap();
        assert!(!workspace.path().join("g").exists());
        assert_eq!(
            fs::read_to_string(workspace.path().join("f")).unwrap(),
            "old\r\n"
        );
    }

    #[test]
    fn apply_patch_updates_multiple_hunks_in_one_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("notes.txt");
        fs::write(&file_path, "one\ntwo\nthree\nfour\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_update",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Update File: notes.txt\n@@\n one\n-two\n+TWO\n three\n@@\n three\n-four\n+FOUR\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Updated file: notes.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        let updated = fs::read_to_string(&file_path).unwrap();
        assert_eq!(updated, "one\nTWO\nthree\nFOUR\n");
    }

    #[test]
    fn apply_patch_creates_new_file() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_add",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Add File: created.txt\n+hello\n+world\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Added file: created.txt"));
                assert!(content.contains("created.txt (+2 -0)"));
                assert!(content.contains("+hello\n+world"));
            }
            _ => panic!("expected tool_result"),
        }

        // Review R13: the last added line had no newline.
        let created = fs::read_to_string(workspace.path().join("created.txt")).unwrap();
        assert_eq!(created, "hello\nworld\n");
    }

    #[test]
    fn end_of_file_markers_and_blank_context_lines_are_accepted() {
        // Review R13: both are common in patches models write.
        let workspace = make_temp_dir("workspace");
        let file = workspace.path().join("f.txt");
        fs::write(&file, "a\n\nb\nend\nend\n").unwrap();
        let patch = "*** Begin Patch\n*** Update File: f.txt\n@@\n a\n\n-b\n+B\n@@\n end\n-end\n+last\n*** End of File\n*** End Patch";
        apply_patch_from_workspace(workspace.path(), &json!({ "patch": patch }), None).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "a\n\nB\nend\nlast\n");

        let wrong =
            "*** Begin Patch\n*** Update File: f.txt\n@@\n-a\n+A\n*** End of File\n*** End Patch";
        let err = apply_patch_from_workspace(workspace.path(), &json!({ "patch": wrong }), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("at the end of 'f.txt'"), "{err}");
    }

    #[test]
    fn a_preview_shows_only_the_changed_lines() {
        // Review R13: a one-line change in a 20 KB file previewed 48 KB.
        let workspace = make_temp_dir("workspace");
        let text = (1..=2_000)
            .map(|n| format!("line {n}\n"))
            .collect::<String>();
        fs::write(workspace.path().join("big.txt"), &text).unwrap();
        let patch = "*** Begin Patch\n*** Update File: big.txt\n@@\n-line 1000\n+line one thousand\n*** End Patch";
        let preview = apply_patch_from_workspace(
            workspace.path(),
            &json!({ "patch": patch, "dry_run": true }),
            None,
        )
        .unwrap();
        assert!(preview.len() < 400, "{}", preview.len());
        assert!(preview.contains("@@ -997,7 +997,7 @@\n line 997"));
        assert!(preview.contains("-line 1000\n+line one thousand"));
    }

    #[test]
    fn apply_patch_deletes_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("remove.txt");
        fs::write(&file_path, "delete me").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_delete",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Delete File: remove.txt\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Deleted file: remove.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        assert!(!file_path.exists());
    }

    #[test]
    fn apply_patch_moves_file_and_updates_contents() {
        let workspace = make_temp_dir("workspace");
        let from_path = workspace.path().join("old.txt");
        fs::write(&from_path, "alpha\nbeta\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_move",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n alpha\n-beta\n+gamma\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Moved file: old.txt -> new.txt"));
                assert!(content.contains("Updated file: new.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        assert!(!from_path.exists());
        let moved = fs::read_to_string(workspace.path().join("new.txt")).unwrap();
        assert_eq!(moved, "alpha\ngamma\n");
    }

    #[test]
    fn apply_patch_dry_run_returns_preview_without_writing() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("preview.txt");
        fs::write(&file_path, "before\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_preview",
            "apply_patch",
            &json!({
                "dry_run": true,
                "patch": "*** Begin Patch\n*** Update File: preview.txt\n@@\n-before\n+after\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Preview: true"));
                assert!(content.contains("--- preview.txt"));
                assert!(content.contains("+++ preview.txt"));
                assert!(content.contains("-before"));
                assert!(content.contains("+after"));
            }
            _ => panic!("expected tool_result"),
        }

        let unchanged = fs::read_to_string(&file_path).unwrap();
        assert_eq!(unchanged, "before\n");
    }

    #[test]
    fn apply_patch_warns_when_git_worktree_is_dirty() {
        let workspace = make_temp_dir("git-dirty-patch");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "dirty\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_dirty_patch",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Add File: another.txt\n+hello\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Added file: another.txt"));
                assert!(content.contains("Warning: git worktree has uncommitted changes"));
            }
            _ => panic!("expected tool_result"),
        }
    }
}
