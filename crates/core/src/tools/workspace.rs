use crate::tools::error::ToolError;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

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
    Ok(canonical)
}

pub(crate) fn resolve_workspace_path_for_create(
    workspace_root: &Path,
    requested_path: &str,
) -> Result<PathBuf, ToolError> {
    let (joined, workspace_canonical) = canonical_workspace_join(workspace_root, requested_path)?;
    match fs::symlink_metadata(&joined) {
        Ok(metadata) => {
            let canonical = match joined.canonicalize() {
                Ok(path) => path,
                Err(e) if metadata.file_type().is_symlink() && e.kind() == ErrorKind::NotFound => {
                    return Err(
                        format!("Path '{}' points to a dangling symlink", requested_path).into(),
                    );
                }
                Err(e) => {
                    return Err(format!("Failed to resolve path '{}': {e}", requested_path).into());
                }
            };
            ensure_inside_workspace(&canonical, &workspace_canonical, requested_path)?;
            return Ok(canonical);
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!("Failed to inspect path '{}': {e}", requested_path).into());
        }
    }

    let mut canonical_ancestor: Option<PathBuf> = None;
    for ancestor in joined.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                let canonical = match ancestor.canonicalize() {
                    Ok(path) => path,
                    Err(e)
                        if metadata.file_type().is_symlink() && e.kind() == ErrorKind::NotFound =>
                    {
                        return Err(format!(
                            "Path '{}' includes a dangling symlink in an ancestor",
                            requested_path
                        )
                        .into());
                    }
                    Err(e) => {
                        return Err(format!(
                            "Failed to resolve parent directory for '{}': {e}",
                            requested_path
                        )
                        .into());
                    }
                };
                canonical_ancestor = Some(canonical);
                break;
            }
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to inspect parent directory for '{}': {e}",
                    requested_path
                )
                .into());
            }
        }
    }

    let canonical_ancestor = canonical_ancestor.ok_or_else(|| {
        format!(
            "Failed to resolve parent directory for path '{}'",
            requested_path
        )
    })?;
    ensure_inside_workspace(&canonical_ancestor, &workspace_canonical, requested_path)?;
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use crate::tools::read_file::read_file_from_workspace;
    use serde_json::json;
    use std::fs;

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
