//! Security regressions use only local fixtures and fake credentials.
use crate::state::{load_json_file, save_json_file, workspace_state_path};
use serde_json::{Value, json};
use std::fs;

#[cfg(unix)]
#[test]
fn file_tools_protect_relocated_git_metadata_and_its_aliases() {
    use crate::workspace::resolve_workspace_mutation_path;
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("repository-data")).unwrap();
    fs::write(
        root.path().join("repository-data/config"),
        "private git config",
    )
    .unwrap();
    symlink("repository-data", root.path().join(".git")).unwrap();
    symlink("repository-data", root.path().join("alias")).unwrap();
    for path in [
        ".git/config",
        "repository-data/config",
        "alias/config",
        ".git/hooks/new",
    ] {
        assert!(
            resolve_workspace_mutation_path(root.path(), path).is_err(),
            "{path}"
        );
    }
    assert!(resolve_workspace_mutation_path(root.path(), "source.rs").is_ok());
}

#[cfg(unix)]
#[test]
fn state_rejects_symlink_directories_files_and_hardlinks() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), root.path().join(".socium")).unwrap();
    assert!(workspace_state_path(root.path(), "conversation.json").is_err());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    fs::remove_file(root.path().join(".socium")).unwrap();
    let path = workspace_state_path(root.path(), "conversation.json").unwrap();
    let target = outside.path().join("private.json");
    fs::write(&target, "{\"secret\":true}").unwrap();
    symlink(&target, &path).unwrap();
    assert!(load_json_file::<Value>(&path).is_err());
    assert!(save_json_file(&path, &json!({"overwrite":true})).is_err());
    fs::remove_file(&path).unwrap();
    fs::hard_link(&target, &path).unwrap();
    assert!(load_json_file::<Value>(&path).is_err());
    assert!(save_json_file(&path, &json!({})).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "{\"secret\":true}");
    let output = root.path().join(".socium/outputs");
    symlink(outside.path(), &output).unwrap();
    assert!(workspace_state_path(root.path(), "outputs/command.stdout").is_err());
}

#[cfg(unix)]
#[test]
fn state_rechecks_replaced_parent_and_enforces_private_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = workspace_state_path(root.path(), "conversation.json").unwrap();
    fs::write(&path, "{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    save_json_file(&path, &json!({"private":true})).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    fs::rename(root.path().join(".socium"), root.path().join("saved-state")).unwrap();
    symlink(outside.path(), root.path().join(".socium")).unwrap();
    assert!(save_json_file(&path, &json!({})).is_err());
    assert!(load_json_file::<Value>(&path).is_err());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn state_rejects_oversized_files_without_reading_them() {
    let root = tempfile::tempdir().unwrap();
    let path = workspace_state_path(root.path(), "large.json").unwrap();
    fs::File::create(&path)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert!(load_json_file::<Value>(&path).is_err());
}

#[cfg(unix)]
#[test]
fn git_queries_do_not_execute_configured_helpers() {
    use crate::test_support::{git_stdout, init_git_repo};
    use crate::tools::git::{
        git_diff_from_workspace, git_log_from_workspace, git_status_from_workspace,
    };
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    fs::write(
        root.path().join(".gitattributes"),
        "tracked.txt filter=probe\n",
    )
    .unwrap();
    git_stdout(
        root.path(),
        &["config", "filter.probe.clean", "touch filter-executed; cat"],
    );
    fs::write(root.path().join("tracked.txt"), "changed content\n").unwrap();
    assert!(git_status_from_workspace(root.path(), &json!({})).is_err());
    assert!(git_diff_from_workspace(root.path(), &json!({})).is_err());
    assert!(!root.path().join("filter-executed").exists());
    git_stdout(root.path(), &["config", "--remove-section", "filter.probe"]);
    git_stdout(
        root.path(),
        &["config", "core.fsmonitor", "touch monitor-executed"],
    );
    git_stdout(
        root.path(),
        &["config", "diff.external", "touch diff-executed"],
    );
    git_status_from_workspace(root.path(), &json!({})).unwrap();
    git_diff_from_workspace(root.path(), &json!({})).unwrap();
    git_log_from_workspace(root.path(), &json!({})).unwrap();
    assert!(!root.path().join("monitor-executed").exists());
    assert!(!root.path().join("diff-executed").exists());
}

#[tokio::test]
async fn provider_client_never_follows_redirects_with_credentials() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let source = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let url = format!("http://{}/messages", source.local_addr().unwrap());
    let redirect = format!("http://{}/capture", target.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut socket, _) = source.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        let count = socket.read(&mut request).unwrap();
        assert!(count > 0);
        write!(socket, "HTTP/1.1 307 Temporary Redirect\r\nLocation: {redirect}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let response = crate::provider_http_client()
        .unwrap()
        .post(url)
        .header("x-api-key", "offline-test-placeholder")
        .body("private prompt")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
    server.join().unwrap();
    assert!(matches!(target.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}
