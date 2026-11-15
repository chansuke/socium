//! Enforcement tests must run on Linux with working user namespaces and bwrap.
//! Missing enforcement is a failure, never a reason to skip the suite.
use super::*;
use crate::{
    TaskControl,
    process::{ProcessOutput, run_observed},
};
use std::{
    fs,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
    time::Duration,
};

fn run(root: &Path, access: WorkspaceAccess, args: &[&str]) -> ProcessOutput {
    run_observed(
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        root,
        Duration::from_secs(5),
        &TaskControl::default(),
        &mut |_, _| Ok(()),
        &ExecutionPolicy::new(root, access).unwrap(),
    )
    .unwrap()
}

fn success(output: ProcessOutput) {
    assert!(
        output.status.success() && !output.timed_out,
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn policy_denies_outside_reads_writes_symlinks_and_wrappers() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("canary");
    fs::write(&secret, "not-a-real-secret").unwrap();
    symlink(&secret, root.path().join("alias")).unwrap();
    fs::write(root.path().join("inside"), "before").unwrap();
    success(run(
        root.path(),
        WorkspaceAccess::Write,
        &["sh", "-c", "echo allowed > inside; cat inside"],
    ));
    success(run(root.path(), WorkspaceAccess::ReadOnly, &["true"]));
    let read = run(root.path(), WorkspaceAccess::ReadOnly, &["cat", "inside"]);
    assert_eq!(read.stdout, b"allowed\n");
    success(read);
    for command in [
        vec!["cat", secret.to_str().unwrap()],
        vec!["cat", "alias"],
        vec!["sed", "-i", "s/secret/changed/", secret.to_str().unwrap()],
        vec!["env", "sh", "-c", "echo changed > alias"],
    ] {
        assert!(
            !run(root.path(), WorkspaceAccess::Write, &command)
                .status
                .success(),
            "{command:?}"
        );
    }
    assert_eq!(fs::read_to_string(secret).unwrap(), "not-a-real-secret");
    // Classification by name is irrelevant: even a workspace binary called
    // 'cat' receives exactly the same policy, and is denied a forbidden write.
    fs::write(root.path().join("cat"), "#!/bin/sh\necho changed > alias\n").unwrap();
    fs::set_permissions(root.path().join("cat"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        !run(root.path(), WorkspaceAccess::Write, &["./cat"])
            .status
            .success()
    );
    assert!(
        !run(
            root.path(),
            WorkspaceAccess::ReadOnly,
            &["sed", "-i", "s/allowed/changed/", "inside"]
        )
        .status
        .success()
    );
    assert!(
        !run(
            root.path(),
            WorkspaceAccess::ReadOnly,
            &["env", "sh", "-c", "touch new"]
        )
        .status
        .success()
    );
    assert_eq!(
        fs::read_to_string(root.path().join("inside")).unwrap(),
        "allowed\n"
    );
}

#[test]
fn state_is_hidden_and_git_is_readonly_except_for_commit_policy() {
    let root = tempfile::tempdir().unwrap();
    crate::test_support::init_git_repo(root.path());
    let state = crate::workspace_state_path(root.path(), "conversation.json").unwrap();
    fs::write(&state, "private conversation").unwrap();
    symlink(".socium", root.path().join("state-alias")).unwrap();
    symlink(".git", root.path().join("metadata")).unwrap();
    for command in [
        "cat .socium/conversation.json",
        "echo tampered > .socium/conversation.json",
        "echo tampered > state-alias/conversation.json",
        "mv .socium old-state",
        "echo tampered > .git/config",
        "echo tampered > metadata/config",
        "mv .git old-git",
    ] {
        assert!(
            !run(root.path(), WorkspaceAccess::Write, &["sh", "-c", command])
                .status
                .success(),
            "{command}"
        );
    }
    success(run(
        root.path(),
        WorkspaceAccess::GitCommit,
        &["sh", "-c", "echo allowed > .git/probe"],
    ));
    assert!(
        !run(
            root.path(),
            WorkspaceAccess::GitCommit,
            &["sh", "-c", "echo denied > .socium/probe"]
        )
        .status
        .success()
    );
    assert_eq!(fs::read_to_string(state).unwrap(), "private conversation");
}

#[test]
fn network_and_kernel_escape_interfaces_are_denied() {
    let root = tempfile::tempdir().unwrap();
    // Execute this test binary inside the sandbox as a nonsecret syscall probe.
    // Copying it keeps the executable inside the declared workspace grant.
    fs::copy(std::env::current_exe().unwrap(), root.path().join("probe")).unwrap();
    let _socket = UnixListener::bind(root.path().join("host.sock")).unwrap();
    let _datagram =
        std::os::unix::net::UnixDatagram::bind(root.path().join("datagram.sock")).unwrap();
    let output = run(
        root.path(),
        WorkspaceAccess::ReadOnly,
        &[
            "./probe",
            "--exact",
            "execution::tests::network_probe",
            "--nocapture",
        ],
    );
    let completed = String::from_utf8_lossy(&output.stdout).contains("sandbox-network-probe-ok");
    success(output);
    assert!(completed);
}

#[test]
fn network_probe() {
    // This helper is also discovered by the outer test harness; the marker is
    // a pathname in a fresh fixture, not an env variable dropped by policy.
    if !Path::new("host.sock").exists() {
        return;
    }
    // A datagram socketpair can still address a host pathname Unix datagram
    // socket. Denying socket() alone does not enforce network/daemon isolation.
    let pair = std::os::unix::net::UnixDatagram::pair();
    if let Ok((socket, _peer)) = &pair {
        assert!(
            socket
                .send_to(b"nonsecret-canary", "datagram.sock")
                .is_err(),
            "socketpair bypassed the host Unix socket policy"
        );
    }
    assert_eq!(pair.unwrap_err().raw_os_error(), Some(libc::EPERM));
    // Stream and seqpacket pairs only reach their own peer; process spawning
    // needs them, with or without flags in the type argument.
    for kind in [
        libc::SOCK_STREAM,
        libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
        libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
    ] {
        let mut fds = [0; 2];
        // SAFETY: fds is a valid two-element buffer for the kernel to fill.
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, kind, 0, fds.as_mut_ptr()) },
            0,
            "socketpair type {kind:#x} must be allowed"
        );
        // SAFETY: both descriptors were just created and are owned here.
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        assert!(std::os::unix::net::UnixStream::connect("host.sock").is_err());
    }
    let mut fds = [0; 2];
    // SAFETY: as above; the call is expected to fail.
    assert_eq!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
                0,
                fds.as_mut_ptr(),
            )
        },
        -1
    );
    for domain in [
        libc::AF_INET,
        libc::AF_INET6,
        libc::AF_UNIX,
        libc::AF_NETLINK,
    ] {
        // SAFETY: no pointers; the expected error creates no descriptor.
        let fd = unsafe { libc::socket(domain, libc::SOCK_STREAM, 0) };
        assert_eq!(fd, -1, "socket domain {domain} must be denied");
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
    }
    assert!(std::os::unix::net::UnixStream::connect("host.sock").is_err());
    assert!(std::net::TcpStream::connect("127.0.0.1:1").is_err());
    assert!(std::net::UdpSocket::bind("127.0.0.1:0").is_err());
    // A null pointer would yield EFAULT if seccomp did not intercept the call.
    assert_eq!(
        unsafe { libc::syscall(libc::SYS_io_uring_setup, 1, std::ptr::null::<u8>()) },
        -1
    );
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
    assert_eq!(unsafe { libc::unshare(libc::CLONE_NEWUSER) }, -1);
    println!("sandbox-network-probe-ok");
}

#[test]
fn environment_allowlist_excludes_credentials_and_loader_overrides() {
    // Isolate the fake parent environment in a fresh test process; never change
    // global environment while the Rust test harness has concurrent threads.
    if std::env::var_os("SOCIUM_ENV_CANARY").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "execution::tests::environment_allowlist_excludes_credentials_and_loader_overrides",
                "--nocapture",
            ])
            .env("SOCIUM_ENV_CANARY", "fake-parent-secret")
            .env("ANTHROPIC_API_KEY", "fake-provider-secret")
            .env("UNLISTED_PROVIDER_TOKEN", "fake-future-provider-secret")
            .env("SSH_AUTH_SOCK", "/tmp/not-a-real-agent")
            .env("GIT_CONFIG_COUNT", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let output = run(root.path(), WorkspaceAccess::Write, &["env"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let env = String::from_utf8(output.stdout).unwrap();
    for excluded in [
        "fake-",
        "API_KEY",
        "TOKEN=",
        "SOCIUM_ENV_CANARY",
        "SSH_AUTH_SOCK",
        "GIT_CONFIG_COUNT",
    ] {
        assert!(!env.contains(excluded), "leaked {excluded}");
    }
    assert!(env.contains("HOME=/socium-home"));
    success(run(
        root.path(),
        WorkspaceAccess::Write,
        &[
            "sh",
            "-c",
            "touch \"$HOME/private\" /tmp/private; test ! -e .socium/conversation.json",
        ],
    ));
    success(run(
        root.path(),
        WorkspaceAccess::Write,
        &[
            "sh",
            "-c",
            "test ! -e \"$HOME/private\"; test ! -e /tmp/private",
        ],
    ));
}

#[test]
fn invalid_setup_does_not_execute_payload_on_host() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let policy = ExecutionPolicy::new(root.path(), WorkspaceAccess::Write).unwrap();
    assert!(
        SandboxExecutor::command(&policy, &["touch".into(), "marker".into()], outside.path())
            .is_err()
    );
    assert!(!outside.path().join("marker").exists());
    // Force an actual bubblewrap setup failure after planning, not a mock:
    // the validated bind source no longer exists when the backend starts.
    let mut command = SandboxExecutor::command(
        &policy,
        &[
            "touch".into(),
            outside.path().join("marker").to_string_lossy().into(),
        ],
        root.path(),
    )
    .unwrap();
    root.close().unwrap();
    let output = command.output().unwrap();
    assert!(!output.status.success());
    assert!(!outside.path().join("marker").exists());
}

#[test]
fn redirected_git_metadata_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    crate::workspace_state_path(root.path(), "conversation.json").unwrap();
    let policy = ExecutionPolicy::new(root.path(), WorkspaceAccess::Write).unwrap();
    for target in [".", ".socium"] {
        symlink(target, root.path().join(".git")).unwrap();
        assert!(SandboxExecutor::command(&policy, &["true".into()], root.path()).is_err());
        fs::remove_file(root.path().join(".git")).unwrap();
    }
    fs::write(root.path().join(".git"), "gitdir: metadata\n").unwrap();
    assert!(SandboxExecutor::command(&policy, &["true".into()], root.path()).is_err());
}

#[test]
fn detached_descendants_die_on_timeout() {
    let root = tempfile::tempdir().unwrap();
    let policy = ExecutionPolicy::new(root.path(), WorkspaceAccess::Write).unwrap();
    let output = run_observed(
        &[
            "sh".into(),
            "-c".into(),
            "setsid sh -c 'sleep 1; echo escaped > detached' & wait".into(),
        ],
        root.path(),
        Duration::from_millis(200),
        &TaskControl::default(),
        &mut |_, _| Ok(()),
        &policy,
    )
    .unwrap();
    assert!(
        output.timed_out,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::thread::sleep(Duration::from_millis(1100));
    assert!(!root.path().join("detached").exists());
}

#[test]
fn approved_git_hooks_inherit_the_sandbox() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    crate::test_support::init_git_repo(root.path());
    let hook = root.path().join(".git/hooks/pre-commit");
    // Host paths are test arguments, not shell interpolation.
    fs::write(&hook, "#!/bin/sh\ncat outside-secret || exit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(outside.path().join("secret"), "fake secret").unwrap();
    symlink(
        outside.path().join("secret"),
        root.path().join("outside-secret"),
    )
    .unwrap();
    let output = run(
        root.path(),
        WorkspaceAccess::GitCommit,
        &["git", "commit", "--allow-empty", "-m", "blocked hook"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("outside-secret"));
    assert_eq!(
        crate::test_support::git_stdout(root.path(), &["log", "-1", "--format=%s"]).trim(),
        "initial commit"
    );
    fs::write(&hook, "#!/bin/sh\necho confined > hook-ran\n").unwrap();
    success(run(
        root.path(),
        WorkspaceAccess::GitCommit,
        &["git", "commit", "--allow-empty", "-m", "allowed hook"],
    ));
    assert!(root.path().join("hook-ran").exists());
}

fn run_with(policy: &ExecutionPolicy, cwd: &Path, args: &[&str], seconds: u64) -> ProcessOutput {
    run_observed(
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        cwd,
        Duration::from_secs(seconds),
        &TaskControl::default(),
        &mut |_, _| Ok(()),
        policy,
    )
    .unwrap()
}

/// The sandbox must run the tools a coding agent verifies with, not only
/// shell utilities: each of these spawns child processes through a
/// socketpair.
#[test]
fn real_toolchains_build_and_spawn_processes_inside_the_sandbox() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("app/src")).unwrap();
    fs::write(
        root.path().join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        root.path().join("app/src/main.rs"),
        "fn main() { println!(\"built-in-sandbox\"); }\n",
    )
    .unwrap();
    let policy = ExecutionPolicy::new(root.path(), WorkspaceAccess::Write).unwrap();
    let app = root.path().join("app");
    let output = run_with(&policy, &app, &["cargo", "run", "--offline", "-q"], 120);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("built-in-sandbox"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    success(output);
    for (program, script, marker) in [
        (
            "python3",
            "import asyncio, subprocess\nasync def main():\n    p = await asyncio.create_subprocess_exec('echo', 'py-async-ok', stdout=subprocess.PIPE)\n    print((await p.communicate())[0].decode().strip())\nasyncio.run(main())",
            "py-async-ok",
        ),
        (
            "node",
            "console.log(require('child_process').execSync('echo node-child-ok').toString().trim())",
            "node-child-ok",
        ),
    ] {
        let flag = if program == "node" { "-e" } else { "-c" };
        let output = run_with(&policy, root.path(), &[program, flag, script], 30);
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(marker),
            "{program}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
