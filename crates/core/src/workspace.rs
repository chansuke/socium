use crate::tool_error::ToolError;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

pub(crate) fn resolve_workspace_command_cwd(
    workspace_root: &Path,
    requested_path: &str,
) -> Result<PathBuf, ToolError> {
    let resolved = resolve_workspace_path(workspace_root, requested_path)?;
    let metadata = fs::metadata(&resolved)
        .map_err(|e| format!("Failed to inspect command cwd '{}': {e}", requested_path))?;
    if !metadata.is_dir() {
        return Err(ToolError::Message(format!(
            "run_command cwd is not a directory: '{}'",
            requested_path
        )));
    }
    Ok(resolved)
}

/// Canonicalizes the workspace root, rejects absolute request paths, and
/// returns the joined candidate path together with the canonical workspace root.
fn canonical_workspace_join(
    workspace_root: &Path,
    requested_path: &str,
) -> Result<(PathBuf, PathBuf), ToolError> {
    let workspace_canonical = workspace_root.canonicalize().map_err(|e| {
        format!(
            "Failed to resolve workspace root '{}': {e}",
            workspace_root.display()
        )
    })?;
    let path = PathBuf::from(requested_path);
    if path.is_absolute() {
        return Err(ToolError::AbsolutePath {
            path: requested_path.to_string(),
        });
    }
    let joined = workspace_canonical.join(path);
    Ok((joined, workspace_canonical))
}

fn ensure_inside_workspace(
    canonical: &Path,
    workspace_canonical: &Path,
    requested_path: &str,
) -> Result<(), ToolError> {
    if !canonical.starts_with(workspace_canonical) {
        return Err(ToolError::OutsideWorkspace {
            path: requested_path.to_string(),
        });
    }
    Ok(())
}

pub(crate) fn resolve_workspace_path(
    workspace_root: &Path,
    requested_path: &str,
) -> Result<PathBuf, ToolError> {
    let (joined, workspace_canonical) = canonical_workspace_join(workspace_root, requested_path)?;
    let canonical = joined
        .canonicalize()
        .map_err(|e| format!("Failed to resolve path '{}': {e}", requested_path))?;
    ensure_inside_workspace(&canonical, &workspace_canonical, requested_path)?;
    // What a file tool reads reaches the model: Git config can hold
    // credentials and Socium's state holds whole past sessions.
    if in_state_dir(&workspace_canonical, &canonical)
        && !in_scratch_dir(&workspace_canonical, &canonical)
        && !in_outputs_dir(&workspace_canonical, &canonical)
    {
        return Err(format!(
            "'{requested_path}' is Git metadata or Socium session state, which file tools do not read; use the git tools"
        )
        .into());
    }
    Ok(canonical)
}

pub(crate) fn resolve_workspace_path_for_create(
    workspace_root: &Path,
    requested_path: &str,
) -> Result<PathBuf, ToolError> {
    let (_, root) = canonical_workspace_join(workspace_root, requested_path)?;
    let mut resolved = root.clone();
    // Resolve existing symlinks before processing `..`. Missing components are
    // normalized too, so callers never create directories outside the boundary.
    // This is path validation, not protection against concurrent symlink swaps.
    for component in Path::new(requested_path).components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                if resolved.exists() && !resolved.is_dir() {
                    return Err("Parent component follows a regular file".into());
                }
                if resolved == root {
                    return Err(ToolError::OutsideWorkspace {
                        path: requested_path.into(),
                    });
                }
                resolved.pop();
            }
            Component::Normal(name) => {
                if resolved.exists() && !resolved.is_dir() {
                    return Err(
                        format!("Parent is not a directory: '{}'", resolved.display()).into(),
                    );
                }
                resolved.push(name);
                match fs::symlink_metadata(&resolved) {
                    Ok(_) => {
                        resolved = resolved.canonicalize().map_err(|e| {
                            format!(
                                "Failed to resolve path '{}' (possibly dangling symlink): {e}",
                                requested_path
                            )
                        })?;
                    }
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(
                            format!("Failed to inspect path '{}': {e}", requested_path).into()
                        );
                    }
                }
            }
            _ => {
                return Err(ToolError::AbsolutePath {
                    path: requested_path.into(),
                });
            }
        }
        ensure_inside_workspace(&resolved, &root, requested_path)?;
    }
    Ok(resolved)
}

pub(crate) fn resolve_workspace_mutation_path(
    root: &Path,
    requested: &str,
) -> Result<PathBuf, ToolError> {
    let resolved = resolve_workspace_path_for_create(root, requested)?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let relative = resolved.strip_prefix(&root).map_err(|e| e.to_string())?;
    if in_scratch_dir(&root, &resolved) {
        return Ok(resolved);
    }
    if matches!(relative.components().next(), Some(Component::Normal(name)) if name == ".git" || name == ".socium")
        || in_state_dir(&root, &resolved)
    {
        return Err(
            "Git metadata and Socium session state cannot be modified by file tools".into(),
        );
    }
    Ok(resolved)
}

/// Inside `.git` or `.socium`, or what either really is. `resolved` is
/// canonical.
fn in_state_dir(root: &Path, resolved: &Path) -> bool {
    [".git", ".socium"].iter().any(|name| {
        resolved.starts_with(root.join(name))
            || root
                .join(name)
                .canonicalize()
                .is_ok_and(|metadata| resolved.starts_with(metadata))
    })
}

/// A file in `.socium/outputs`, where commands' saved output is read back.
fn in_outputs_dir(root: &Path, resolved: &Path) -> bool {
    resolved.parent() == Some(&root.join(".socium/outputs"))
}

/// Inside `.socium/scratch`, reached without symlinks. `resolved` is
/// canonical, so a link that leads back out of it is not inside.
fn in_scratch_dir(root: &Path, resolved: &Path) -> bool {
    let state = root.join(".socium");
    let scratch = state.join("scratch");
    let real_dir = |path: &Path| {
        fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
    };
    resolved.starts_with(&scratch) && resolved != scratch && real_dir(&state) && real_dir(&scratch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use crate::tools::read_file::read_file_from_workspace;
    use serde_json::json;
    use std::fs;

    #[test]
    fn create_normalizes_missing_components_without_side_effects() {
        let workspace = make_temp_dir("workspace");
        assert!(
            resolve_workspace_path_for_create(workspace.path(), "missing/../../outside").is_err()
        );
        assert!(!workspace.path().join("missing").exists());
        assert_eq!(
            resolve_workspace_path_for_create(workspace.path(), "missing/../safe/file").unwrap(),
            workspace.path().join("safe/file")
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_resolves_symlinks_before_parent_components() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir_all(workspace.path().join("a/b")).unwrap();
        std::os::unix::fs::symlink("a/b", workspace.path().join("link")).unwrap();
        assert_eq!(
            resolve_workspace_path_for_create(workspace.path(), "link/../new").unwrap(),
            workspace.path().join("a/new")
        );
    }

    #[test]
    fn resolve_rejects_relative_escape_path() {
        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();
        let escape = format!(
            "../{}/secret.txt",
            outside.path().file_name().unwrap().to_string_lossy()
        );

        let result = resolve_workspace_path(workspace.path(), &escape);

        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("outside the workspace")
        );
    }

    #[test]
    fn resolve_accepts_dot_components_inside_workspace() {
        let workspace = make_temp_dir("workspace");
        let dir = workspace.path().join("dir");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file.txt");
        fs::write(&file, "inside").unwrap();

        let resolved = resolve_workspace_path(workspace.path(), "dir/./../dir/file.txt").unwrap();

        assert_eq!(resolved, file.canonicalize().unwrap());
    }

    #[test]
    fn resolve_fails_for_nonexistent_path() {
        let workspace = make_temp_dir("workspace");

        let result = resolve_workspace_path(workspace.path(), "missing.txt");

        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .starts_with("Failed to resolve path")
        );
    }

    #[test]
    fn resolve_rejects_absolute_path() {
        let workspace = make_temp_dir("workspace");
        let inside_file = workspace.path().join("inside.txt");
        fs::write(&inside_file, "inside").unwrap();

        let result = resolve_workspace_path(workspace.path(), &inside_file.to_string_lossy());

        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("Absolute paths are not allowed")
        );
    }

    #[test]
    fn resolve_for_create_accepts_missing_file_in_workspace() {
        let workspace = make_temp_dir("workspace");

        let resolved =
            resolve_workspace_path_for_create(workspace.path(), "newdir/nested/missing.txt")
                .unwrap();

        assert_eq!(resolved, workspace.path().join("newdir/nested/missing.txt"));
    }

    #[test]
    fn resolve_for_create_rejects_escape_path() {
        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let escape = format!(
            "../{}/x.txt",
            outside.path().file_name().unwrap().to_string_lossy()
        );

        let result = resolve_workspace_path_for_create(workspace.path(), &escape);

        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("outside the workspace")
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_for_create_rejects_dangling_symlink_path() {
        use std::os::unix::fs::symlink;

        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let missing_target = outside.path().join("missing.txt");
        let link = workspace.path().join("link.txt");
        symlink(&missing_target, &link).unwrap();

        let result = resolve_workspace_path_for_create(workspace.path(), "link.txt");

        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("dangling symlink")
        );
    }

    #[test]
    fn file_tools_may_change_the_scratch_directory_but_no_other_state() {
        let workspace = make_temp_dir("scratch");
        let root = workspace.path();
        crate::state::workspace_state_path(root, "conversation.json").unwrap();
        fs::write(root.join(".socium/conversation.json"), "private").unwrap();
        crate::scratch_dir(root).unwrap();

        let scratch = root.canonicalize().unwrap().join(".socium/scratch");
        for path in [".socium/scratch/check.py", ".socium/scratch/new/deeper.py"] {
            assert!(
                resolve_workspace_mutation_path(root, path)
                    .unwrap()
                    .starts_with(&scratch),
                "{path}"
            );
        }
        for path in [
            ".socium/conversation.json",
            ".socium/scratch/../conversation.json",
            ".socium/new.json",
            ".socium",
            ".socium/scratch",
        ] {
            assert!(
                resolve_workspace_mutation_path(root, path).is_err(),
                "{path}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn file_tools_cannot_read_git_or_session_state() {
        use std::os::unix::fs::symlink;

        let workspace = make_temp_dir("state_reads");
        let root = workspace.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/config"), "[remote]").unwrap();
        crate::state::workspace_state_path(root, "conversation.json").unwrap();
        fs::write(root.join(".socium/conversation.json"), "private").unwrap();
        fs::create_dir_all(root.join(".socium/outputs")).unwrap();
        fs::write(root.join(".socium/outputs/1-2-0.stdout"), "out").unwrap();
        crate::scratch_dir(root).unwrap();
        fs::write(root.join(".socium/scratch/probe.py"), "x").unwrap();
        symlink(".git/config", root.join("config-link")).unwrap();
        symlink(".socium", root.join("state-link")).unwrap();

        for path in [
            ".git",
            ".git/config",
            ".socium",
            ".socium/conversation.json",
            ".socium/outputs/../conversation.json",
            "config-link",
            "state-link/conversation.json",
        ] {
            assert!(resolve_workspace_path(root, path).is_err(), "{path}");
        }
        // Saved command output and the scratch directory are the model's.
        for path in [".socium/outputs/1-2-0.stdout", ".socium/scratch/probe.py"] {
            assert!(resolve_workspace_path(root, path).is_ok(), "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_scratch_directory_is_still_state() {
        use std::os::unix::fs::symlink;

        let workspace = make_temp_dir("scratch_link");
        let root = workspace.path();
        crate::state::workspace_state_path(root, "conversation.json").unwrap();
        fs::write(root.join(".socium/conversation.json"), "private").unwrap();
        let scratch = crate::scratch_dir(root).unwrap();
        symlink("../conversation.json", scratch.join("link.json")).unwrap();
        symlink("..", scratch.join("up")).unwrap();

        for path in [".socium/scratch/link.json", ".socium/scratch/up/new.json"] {
            assert!(
                resolve_workspace_mutation_path(root, path).is_err(),
                "{path}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_outside_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();
        let link = workspace.path().join("link.txt");
        symlink(&outside_file, &link).unwrap();

        let output = read_file_from_workspace(workspace.path(), &json!({"path": "link.txt"}));

        assert!(output.is_err());
        assert!(
            output
                .err()
                .unwrap()
                .to_string()
                .contains("outside the workspace")
        );
    }
}
