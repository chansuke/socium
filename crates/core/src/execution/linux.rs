use super::{ExecutionPolicy, NetworkAccess, WorkspaceAccess};
use std::{
    env, fs, io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{fs::PermissionsExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

// These are public runtime installations, not arbitrary user PATH directories.
// In particular, never mount $HOME, /etc, /run, or the host /tmp wholesale.
const RUNTIMES: &[&str] = &["/usr", "/bin", "/sbin", "/lib", "/lib64", "/nix/store"];
const HOME: &str = "/socium-home";

pub(super) fn runtime_path(path: &Path) -> bool {
    RUNTIMES.iter().any(|root| path.starts_with(root))
}

/// System runtime directories from the launcher's PATH. Only these can
/// provide the sandbox backend itself.
fn system_search_path() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for path in env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .chain([PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
    {
        if path.is_absolute()
            && let Ok(path) = path.canonicalize()
            && runtime_path(&path)
            && !paths.contains(&path)
        {
            paths.push(path);
        }
    }
    paths
}

/// The command PATH: the system runtime directories, then the user's
/// `sandbox.path` entries that lie in a mounted directory.
fn search_path(policy: &ExecutionPolicy) -> Vec<PathBuf> {
    let mut paths = system_search_path();
    for path in &policy.path {
        if (runtime_path(path) || mounted_read_only(policy, path)) && !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    paths
}

fn mounted_read_only(policy: &ExecutionPolicy, path: &Path) -> bool {
    policy
        .read_only
        .iter()
        .any(|mount| path.starts_with(&mount.source))
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn resolve(program: &str, cwd: &Path, paths: &[PathBuf]) -> io::Result<PathBuf> {
    let candidate = if program.contains('/') {
        cwd.join(program)
    } else {
        paths.iter().map(|p| p.join(program)).find(|p| executable(p))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound,
                format!("sandbox executable not found in system runtime PATH: {program}; use ./ for workspace programs")))?
    };
    let path = candidate.canonicalize()?;
    if !executable(&path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "not an executable file",
        ));
    }
    Ok(path)
}

fn supported_backend_version(stdout: &[u8]) -> bool {
    let Ok(output) = std::str::from_utf8(stdout) else {
        return false;
    };
    let Some(version) = output.trim().strip_prefix("bubblewrap ") else {
        return false;
    };
    let Ok(parts) = version
        .split('.')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
    else {
        return false;
    };
    parts.len() == 3 && (parts[0], parts[1], parts[2]) >= (0, 12, 0)
}

fn verify_backend(backend: &Path) -> io::Result<()> {
    if fs::metadata(backend)?.permissions().mode() & 0o6000 != 0 {
        return Err(io::Error::other(
            "setuid/setgid bubblewrap is not supported",
        ));
    }
    // This is trusted backend bootstrap, not a tool command. No workspace
    // paths or inherited environment are passed to its version query.
    let output = Command::new(backend)
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() || !supported_backend_version(&output.stdout) {
        return Err(io::Error::other(
            "bubblewrap >= 0.12.0 is required (CVE-2026-87766); use nix develop / nix run or upgrade the system backend; no host fallback",
        ));
    }
    Ok(())
}

pub(super) fn command(
    policy: &ExecutionPolicy,
    argv: &[String],
    cwd: &Path,
) -> io::Result<Command> {
    if policy.network != NetworkAccess::Deny {
        return Err(io::Error::other("unsupported sandbox network policy"));
    }
    let root = &policy.workspace;
    // Prevent a workspace mount from replacing the sandbox's runtime or
    // isolation mounts, or exposing an entire host tree through an ancestor.
    for reserved in RUNTIMES.iter().copied().chain(["/proc", "/dev", HOME]) {
        let reserved = Path::new(reserved);
        if root.starts_with(reserved) || reserved.starts_with(root) {
            return Err(io::Error::other(
                "workspace overlaps a reserved sandbox path",
            ));
        }
    }
    if root == Path::new("/tmp") {
        return Err(io::Error::other(
            "use a project directory, not /tmp itself, as the workspace",
        ));
    }
    let cwd = cwd.canonicalize()?;
    if !cwd.starts_with(root) {
        return Err(io::Error::other("sandbox cwd is outside the workspace"));
    }
    for mount in &policy.read_only {
        for path in std::iter::once(&mount.source).chain(&mount.alias) {
            if overlaps_reserved(path) {
                return Err(io::Error::other(
                    "a sandbox.read_only directory overlaps a reserved sandbox path",
                ));
            }
        }
    }
    let paths = search_path(policy);
    let program = resolve(
        argv.first()
            .ok_or_else(|| io::Error::other("empty command"))?,
        &cwd,
        &paths,
    )?;
    if !program.starts_with(root) && !runtime_path(&program) && !mounted_read_only(policy, &program)
    {
        return Err(io::Error::other(
            "executable is outside the workspace, the system runtimes and sandbox.read_only",
        ));
    }
    // The backend must itself come from a system installation; ./bwrap or a
    // project-controlled or user-added PATH entry can never replace the
    // sandbox boundary.
    let backend = resolve("bwrap", Path::new("/"), &system_search_path()).map_err(|e| {
        io::Error::other(format!(
            "Linux sandbox requires bubblewrap (bwrap): {e}; no host fallback"
        ))
    })?;
    if !runtime_path(&backend) {
        return Err(io::Error::other("bubblewrap is not in a system runtime"));
    }
    verify_backend(&backend)?;
    let filter = network_filter()?;
    let mut cmd = Command::new(backend);
    cmd.env_clear().args([
        "--unshare-all",
        "--unshare-user",
        "--die-with-parent",
        "--new-session",
        "--disable-userns",
        "--cap-drop",
        "ALL",
    ]);
    for runtime in RUNTIMES {
        if Path::new(runtime).exists() {
            cmd.arg("--ro-bind").arg(runtime).arg(runtime);
        }
    }
    if Path::new("/etc/ld.so.cache").is_file() {
        cmd.args(["--ro-bind", "/etc/ld.so.cache", "/etc/ld.so.cache"]);
    }
    cmd.args([
        "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--tmpfs", HOME,
    ]);
    // After the fresh /tmp, which would hide them, and before the workspace,
    // so a listed ancestor of the workspace cannot make it read-only or
    // reveal the state directory.
    for mount in &policy.read_only {
        cmd.arg("--ro-bind").arg(&mount.source).arg(&mount.source);
        if let Some(alias) = &mount.alias {
            cmd.arg("--ro-bind").arg(&mount.source).arg(alias);
        }
    }
    cmd.arg(if policy.access == WorkspaceAccess::ReadOnly {
        "--ro-bind"
    } else {
        "--bind"
    })
    .arg(root)
    .arg(root);

    // Persisted conversation, approval records and output artifacts belong to
    // the parent. The child sees an empty read-only directory in their place.
    // Creating the state directory here also protects it on the very first run.
    crate::agent::state::workspace_state_path(root, "sandbox")?;
    cmd.arg("--tmpfs")
        .arg(root.join(".socium"))
        .arg("--remount-ro")
        .arg(root.join(".socium"));

    let git = root.join(".git");
    match git.symlink_metadata() {
        // A symlink itself could be unlinked even when its target is mounted
        // read-only; gitfiles/commondir can also redirect to other metadata.
        // Support ordinary checkouts only, rather than pretend those layouts
        // inherit the same metadata protection.
        Ok(meta) if !meta.file_type().is_dir() || git.join("commondir").exists() => {
            return Err(io::Error::other(
                "sandbox requires an ordinary .git directory; linked worktrees, gitfiles and symlinked metadata are unsupported",
            ));
        }
        Ok(_) => {
            if policy.access != WorkspaceAccess::GitCommit {
                cmd.arg("--ro-bind").arg(&git).arg(&git);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    // Files a command nobody reviewed must not change or read. Each was found
    // inside the workspace; a path that left it since is refused.
    for (paths, source) in [(&policy.frozen, None), (&policy.hidden, Some("/dev/null"))] {
        for path in paths {
            let canonical = path.canonicalize()?;
            if !canonical.starts_with(root) {
                return Err(io::Error::other(
                    "a protected path left the workspace before the command started",
                ));
            }
            cmd.arg("--ro-bind")
                .arg(source.map_or(canonical.as_path(), Path::new))
                .arg(&canonical);
        }
    }
    cmd.arg("--chdir").arg(&cwd);
    cmd.env("PATH", env::join_paths(&paths).map_err(io::Error::other)?)
        .env("HOME", HOME)
        .env("TMPDIR", "/tmp")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("TERM", "dumb")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1");
    // Intentionally no inherited environment, including credentials, proxies,
    // agent sockets, loader variables and Git execution/configuration overrides.
    // The user's own sandbox.env settings are the only additions; settings
    // validation keeps them away from PATH, HOME, TMPDIR and GIT_*.
    for (name, value) in &policy.env {
        cmd.env(name, value);
    }
    cmd.arg("--seccomp")
        .arg(filter.as_raw_fd().to_string())
        // Preserve argv[0] when pinning a symlink to its canonical executable:
        // Nix coreutils and other multicall binaries dispatch by this name.
        .arg("--argv0")
        .arg(&argv[0])
        .arg("--")
        .arg(program)
        .args(&argv[1..]);
    // SAFETY: only async-signal-safe fcntl is used after fork. The owned filter
    // stays alive with Command until spawn and bubblewrap closes it after use.
    unsafe {
        cmd.pre_exec(move || {
            if libc::fcntl(filter.as_raw_fd(), libc::F_SETFD, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(cmd)
}

/// Whether mounting `path` would replace or cover a mount the sandbox makes
/// itself. A directory inside `/tmp` is fine, since it is mounted after the
/// fresh `/tmp`; `/tmp` itself or an ancestor of it is not.
fn overlaps_reserved(path: &Path) -> bool {
    ["/proc", "/dev", HOME]
        .iter()
        .any(|reserved| path.starts_with(reserved) || Path::new(reserved).starts_with(path))
        || Path::new("/tmp").starts_with(path)
}

/// Network namespaces alone do not isolate pathname Unix sockets (e.g. a
/// daemon socket checked into/mounted in a workspace). Refuse socket creation,
/// including socketpair (whose Unix datagrams can reach host pathname sockets),
/// and io_uring's alternate socket API. Reject alternate ABIs.
fn network_filter() -> io::Result<fs::File> {
    use std::io::{Seek, Write};
    #[cfg(target_arch = "x86_64")]
    let arch = 0xc000003e;
    #[cfg(target_arch = "aarch64")]
    let arch = 0xc00000b7;
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "sandbox seccomp requires x86_64 or aarch64",
    ));

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        // Classic BPF over struct seccomp_data: nr at byte 0, arch at byte 4.
        let mut instructions: Vec<(u16, u8, u8, u32)> = vec![
            (0x20, 0, 0, 4),
            (0x15, 1, 0, arch),
            (0x06, 0, 0, 0x80000000), // KILL_PROCESS for an unknown ABI
            (0x20, 0, 0, 0),
            (0x35, 0, 1, 0x40000000), // also reject x32 syscall numbering
            (0x06, 0, 0, 0x80000000),
        ];
        // socketpair yields a connected pair with no address. A stream or
        // seqpacket pair can only talk to its own peer, and process spawning
        // (Rust's std::process, Python's asyncio, Node's child_process) needs
        // one. A datagram pair could still send to a host pathname socket
        // with sendto, so that type stays denied. Flags in the type argument
        // (SOCK_CLOEXEC, SOCK_NONBLOCK) are masked off before comparing.
        instructions.extend([
            (0x15, 0, 6, libc::SYS_socketpair as u32),
            (0x20, 0, 0, 24), // A = low word of args[1], the socket type
            (0x54, 0, 0, 0xf),
            (0x15, 2, 0, libc::SOCK_STREAM as u32),
            (0x15, 1, 0, libc::SOCK_SEQPACKET as u32),
            (0x06, 0, 0, 0x00050000 | libc::EPERM as u32),
            (0x06, 0, 0, 0x7fff0000),
        ]);
        for syscall in [
            libc::SYS_socket,
            libc::SYS_io_uring_setup,
            libc::SYS_keyctl,
            libc::SYS_add_key,
            libc::SYS_request_key,
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
            libc::SYS_pidfd_getfd,
            libc::SYS_bpf,
            libc::SYS_perf_event_open,
        ] {
            instructions.push((0x15, 0, 1, syscall as u32));
            instructions.push((0x06, 0, 0, 0x00050000 | libc::EPERM as u32));
        }
        instructions.push((0x06, 0, 0, 0x7fff0000)); // ALLOW
        // SAFETY: static nul-terminated name; a successful syscall returns a
        // newly owned descriptor. No host pathname is used for the policy.
        let fd = unsafe { libc::memfd_create(c"socium-seccomp".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut file = unsafe { fs::File::from_raw_fd(fd) };
        for (code, jt, jf, k) in instructions {
            file.write_all(&code.to_ne_bytes())?;
            file.write_all(&[jt, jf])?;
            file.write_all(&k.to_ne_bytes())?;
        }
        file.rewind()?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::supported_backend_version;

    #[test]
    fn old_or_unrecognized_backends_are_refused() {
        for output in [
            "bubblewrap 0.9.0\n",
            "bubblewrap 0.11.2\n",
            "bubblewrap 0.12.0-extra",
            "0.12.0",
            "bubblewrap 0.12",
            "",
        ] {
            assert!(!supported_backend_version(output.as_bytes()), "{output}");
        }
        for output in [
            "bubblewrap 0.12.0\n",
            "bubblewrap 0.12.1\n",
            "bubblewrap 1.0.0\n",
        ] {
            assert!(supported_backend_version(output.as_bytes()), "{output}");
        }
    }
}
