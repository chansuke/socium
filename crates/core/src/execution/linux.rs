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

use crate::host::{RUNTIME_ROOTS, is_runtime_path};
const HOME: &str = "/socium-home";

/// System runtime directories from the launcher's PATH. Only these can
/// provide the sandbox backend itself.
fn system_search_path() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for path in env::split_paths(&crate::host::search_path().unwrap_or_default())
        .chain([PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
    {
        if path.is_absolute()
            && let Ok(path) = path.canonicalize()
            && is_runtime_path(&path)
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
        if (is_runtime_path(path) || mounted_read_only(policy, path)) && !paths.contains(path) {
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

/// Whether a command named `program` resolves in the system runtime PATH,
/// as the read-only helpers such as rg and git look it up.
pub(super) fn finds_program(program: &str) -> bool {
    finds_program_in(program, &system_search_path())
}

pub(super) fn finds_program_in(program: &str, paths: &[PathBuf]) -> bool {
    paths.iter().any(|path| executable(&path.join(program)))
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `program` as the sandbox runs it: by its path, or found on `paths`,
/// links followed. `hint` ends the error when it is not found.
fn resolve(program: &str, cwd: &Path, paths: &[PathBuf], hint: &str) -> io::Result<PathBuf> {
    let candidate = if program.contains('/') {
        cwd.join(program)
    } else {
        paths
            .iter()
            .map(|p| p.join(program))
            .find(|p| executable(p))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("sandbox executable not found in system runtime PATH: {program}{hint}"),
                )
            })?
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

/// Which file a backend path named when its version was checked.
#[derive(PartialEq)]
struct BackendIdentity {
    path: PathBuf,
    device: u64,
    inode: u64,
    modified: (i64, i64),
}

fn verify_backend(backend: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    use std::sync::Mutex;
    /// The binary whose version passed, so each command does not run
    /// `--version` again. A replaced binary has another identity.
    static VERIFIED: Mutex<Option<BackendIdentity>> = Mutex::new(None);

    let metadata = fs::metadata(backend)?;
    if metadata.permissions().mode() & 0o6000 != 0 {
        return Err(io::Error::other(
            "setuid/setgid bubblewrap is not supported",
        ));
    }
    let identity = BackendIdentity {
        path: backend.to_path_buf(),
        device: metadata.dev(),
        inode: metadata.ino(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
    };
    let mut verified = VERIFIED.lock().unwrap_or_else(|e| e.into_inner());
    if verified.as_ref() == Some(&identity) {
        return Ok(());
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
    *verified = Some(identity);
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
    if let Some(reason) = workspace_refusal(root) {
        return Err(io::Error::other(reason));
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
        // The workspace, its state and its secrets are masked only at their
        // canonical path, so a second path to the same directory would show
        // them unmasked.
        if let Some(alias) = &mount.alias
            && (root.starts_with(&mount.source) || mount.source.starts_with(root))
        {
            return Err(io::Error::other(format!(
                "sandbox.read_only lists {} through a symlink, and its target {} overlaps the workspace; list the target instead",
                alias.display(),
                mount.source.display()
            )));
        }
    }
    let paths = search_path(policy);
    let name = argv
        .first()
        .ok_or_else(|| io::Error::other("empty command"))?;
    // Resolved once, here: what was checked is what runs.
    let program = if policy.workspace_programs {
        resolve(name, &cwd, &paths, "; use ./ for workspace programs")?
    } else {
        resolve(
            name,
            &cwd,
            &paths,
            "; give an absolute path in a system runtime or a read_only directory",
        )?
    };
    let in_workspace = policy.workspace_programs && program.starts_with(root);
    if !in_workspace && !is_runtime_path(&program) && !mounted_read_only(policy, &program) {
        return Err(io::Error::other(if policy.workspace_programs {
            "executable is outside the workspace, the system runtimes and sandbox.read_only"
                .to_string()
        } else {
            format!(
                "`{name}` is {}, which is in neither a system runtime such as /nix/store or /usr nor a read_only directory",
                program.display()
            )
        }));
    }
    // The backend must itself come from a system installation; ./bwrap or a
    // project-controlled or user-added PATH entry can never replace the
    // sandbox boundary.
    let backend = resolve("bwrap", Path::new("/"), &system_search_path(), "").map_err(|e| {
        io::Error::other(format!(
            "Linux sandbox requires bubblewrap (bwrap): {e}; no host fallback"
        ))
    })?;
    if !is_runtime_path(&backend) {
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
    for runtime in RUNTIME_ROOTS {
        let path = Path::new(runtime);
        // On a merged-/usr system /bin, /lib and the rest link into /usr.
        // The same link costs no mount, and each mount adds about a
        // millisecond to every command.
        if let Ok(target) = fs::read_link(path)
            && let Ok(resolved) = path.canonicalize()
            && RUNTIME_ROOTS.iter().any(|other| {
                resolved.starts_with(other)
                    && fs::symlink_metadata(other).is_ok_and(|meta| meta.is_dir())
            })
        {
            cmd.arg("--symlink").arg(target).arg(runtime);
        } else if path.exists() {
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
    // reveal the state directory. Each is mounted from a descriptor opened
    // where the settings found it, so a link put in its place or in one of
    // its parents since cannot redirect the mount.
    let mut mounts = Vec::new();
    for mount in &policy.read_only {
        let pinned = pin_mount_source(&mount.source)?;
        // bubblewrap closes a descriptor once it is mounted.
        let alias = match &mount.alias {
            Some(alias) => Some((pinned.try_clone()?, alias)),
            None => None,
        };
        cmd.arg("--ro-bind-fd")
            .arg(pinned.as_raw_fd().to_string())
            .arg(&mount.source);
        mounts.push(pinned);
        if let Some((again, alias)) = alias {
            cmd.arg("--ro-bind-fd")
                .arg(again.as_raw_fd().to_string())
                .arg(alias);
            mounts.push(again);
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
    // the parent. The child sees a read-only directory in their place, holding
    // only the writable scratch directory. Creating the state directory here
    // also protects it on the very first run.
    let scratch = crate::state::scratch_dir(root)?;
    cmd.arg("--tmpfs")
        .arg(root.join(".socium"))
        .arg(if policy.access == WorkspaceAccess::ReadOnly {
            "--ro-bind"
        } else {
            "--bind"
        })
        .arg(&scratch)
        .arg(root.join(".socium/scratch"))
        .arg("--remount-ro")
        .arg(root.join(".socium"));

    let git = root.join(".git");
    match git.symlink_metadata() {
        // A symlink itself could be unlinked even when its target is mounted
        // read-only, and commondir in a plain directory redirects to other
        // metadata. Neither layout gets the same protection, so both fail.
        Ok(meta) if meta.file_type().is_dir() && !git.join("commondir").exists() => {
            if policy.access != WorkspaceAccess::GitCommit {
                cmd.arg("--ro-bind").arg(&git).arg(&git);
            }
        }
        Ok(meta) if meta.file_type().is_file() => {
            // A linked worktree: its metadata lives in the main checkout's
            // Git directory, which is mounted like an ordinary `.git`. The
            // gitfile itself is never writable, so it cannot be redirected.
            let common = linked_worktree_common_dir(root, &git)?;
            let mode = if policy.access == WorkspaceAccess::GitCommit {
                "--bind"
            } else {
                "--ro-bind"
            };
            cmd.arg(mode).arg(&common).arg(&common);
            cmd.arg("--ro-bind").arg(&git).arg(&git);
        }
        Ok(_) => {
            return Err(io::Error::other(
                "sandbox requires an ordinary .git directory or a linked worktree; symlinked metadata, submodule gitfiles and redirected layouts are unsupported",
            ));
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
    // validation keeps them away from PATH, HOME, TMPDIR and GIT_*. bwrap sets
    // them in the child, so a loader variable such as LD_PRELOAD never reaches
    // bwrap itself, which runs on the host before any isolation exists. They
    // are read from a descriptor, not argv, because any user on the host can
    // read a process's command line but not its environment.
    let env_args = user_env_args(&policy.env)?;
    if let Some(file) = &env_args {
        cmd.arg("--args").arg(file.as_raw_fd().to_string());
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
    // SAFETY: only async-signal-safe fcntl is used after fork. The owned files
    // stay alive with Command until spawn and bubblewrap closes them after use.
    unsafe {
        cmd.pre_exec(move || {
            for fd in std::iter::once(filter.as_raw_fd())
                .chain(env_args.as_ref().map(AsRawFd::as_raw_fd))
                .chain(mounts.iter().map(AsRawFd::as_raw_fd))
            {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    Ok(cmd)
}

/// A descriptor for the directory at `source`, a path the settings found
/// canonical, once it is checked to still be there: a link swapped in for
/// it or for one of its parents leads elsewhere, and is refused.
fn pin_mount_source(source: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let pinned = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH)
        .open(source)?;
    let opened = fs::read_link(format!("/proc/self/fd/{}", pinned.as_raw_fd()))?;
    if opened != source {
        return Err(io::Error::other(format!(
            "the read_only directory {} now leads to {}; it changed since the settings were read",
            source.display(),
            opened.display()
        )));
    }
    Ok(pinned)
}

/// `--setenv` arguments for the user's `sandbox.env`, NUL-separated in an
/// anonymous file for bwrap's `--args`; `None` when there are none.
fn user_env_args(env: &[(String, String)]) -> io::Result<Option<fs::File>> {
    use std::io::{Seek, Write};
    if env.is_empty() {
        return Ok(None);
    }
    let mut args = Vec::new();
    for (name, value) in env {
        if name.contains('\0') || value.contains('\0') {
            return Err(io::Error::other("sandbox.env cannot contain a NUL byte"));
        }
        for part in ["--setenv", name, value] {
            args.extend_from_slice(part.as_bytes());
            args.push(0);
        }
    }
    // SAFETY: static nul-terminated name; a successful syscall returns a
    // newly owned descriptor. No host pathname holds the values.
    let fd = unsafe { libc::memfd_create(c"socium-env".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by memfd_create and nothing else owns
    // it, so the file takes sole ownership and closes it.
    let mut file = unsafe { fs::File::from_raw_fd(fd) };
    file.write_all(&args)?;
    file.rewind()?;
    Ok(Some(file))
}

/// The main checkout's Git directory for a linked worktree at `root`, after
/// checking every link `git worktree add` creates: the gitfile names a
/// directory under `<common>/worktrees/`, that directory's `gitdir` file
/// points back to this gitfile, and its `commondir` names `<common>`. A
/// gitfile written by hand (or by a hostile repository) to expose some other
/// directory fails these checks, so nothing outside a real Git directory is
/// ever mounted.
fn linked_worktree_common_dir(root: &Path, gitfile: &Path) -> io::Result<PathBuf> {
    let refused = |reason: &str| {
        io::Error::other(format!(
            "sandbox supports a gitfile only for a linked worktree created by `git worktree add`: {reason}"
        ))
    };
    let read = |path: &Path| -> io::Result<String> {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        const MAX_BYTES: u64 = 4096;
        // Without O_NONBLOCK, opening a FIFO waits for a writer that may
        // never come, and startup with it. Only a regular file is read.
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("too large for Git metadata"));
        }
        let text = String::from_utf8(bytes).map_err(io::Error::other)?;
        Ok(text.trim().to_string())
    };
    let gitfile_text = read(gitfile)?;
    let target = gitfile_text
        .strip_prefix("gitdir:")
        .map(str::trim)
        .ok_or_else(|| refused("`.git` is not a gitfile"))?;
    let gitdir = root
        .join(target)
        .canonicalize()
        .map_err(|_| refused("its gitdir does not exist"))?;
    let back =
        PathBuf::from(read(&gitdir.join("gitdir")).map_err(|_| refused("no gitdir backlink"))?);
    let back = if back.is_absolute() {
        back
    } else {
        gitdir.join(back)
    };
    if back.canonicalize().ok().as_deref() != Some(gitfile.canonicalize()?.as_path()) {
        return Err(refused("its gitdir belongs to another worktree"));
    }
    let common = read(&gitdir.join("commondir")).map_err(|_| refused("no commondir"))?;
    let common = gitdir
        .join(common)
        .canonicalize()
        .map_err(|_| refused("its commondir does not exist"))?;
    if gitdir.parent() != Some(common.join("worktrees").as_path())
        || !common.join("objects").is_dir()
        || !common.join("HEAD").is_file()
    {
        return Err(refused("its metadata is not laid out as Git creates it"));
    }
    // Inside, it would be writable through the workspace; around it, its
    // mount, made after the workspace's, would cover `.socium` and the
    // hidden files again.
    if common.starts_with(root) || root.starts_with(&common) {
        return Err(refused(
            "its metadata is inside the worktree or contains it",
        ));
    }
    if overlaps_reserved(&common)
        || RUNTIME_ROOTS
            .iter()
            .any(|runtime| common.starts_with(runtime) || Path::new(runtime).starts_with(&common))
    {
        return Err(refused("its metadata overlaps a reserved sandbox path"));
    }
    Ok(common)
}

/// Why `root` cannot be the workspace: its mount would replace the sandbox's
/// runtime or isolation mounts, or expose an entire host tree through an
/// ancestor.
fn workspace_refusal(root: &Path) -> Option<&'static str> {
    for reserved in RUNTIME_ROOTS.iter().copied().chain(["/proc", "/dev", HOME]) {
        let reserved = Path::new(reserved);
        if root.starts_with(reserved) || reserved.starts_with(root) {
            return Some("workspace overlaps a reserved sandbox path");
        }
    }
    if root == Path::new("/tmp") {
        return Some("use a project directory, not /tmp itself, as the workspace");
    }
    None
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
        // SAFETY: `fd` is the descriptor memfd_create just returned, owned by
        // nothing else.
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
mod properties {
    use super::{HOME, RUNTIME_ROOTS, overlaps_reserved, workspace_refusal};
    use proptest::prelude::*;
    use std::path::{Path, PathBuf};

    fn absolute_path() -> impl Strategy<Value = PathBuf> {
        let part = prop::sample::select(vec![
            "usr",
            "bin",
            "lib",
            "lib64",
            "nix",
            "store",
            "proc",
            "dev",
            "tmp",
            "socium-home",
            "home",
            "project",
        ]);
        prop::collection::vec(part, 0..4)
            .prop_map(|parts| PathBuf::from(format!("/{}", parts.join("/"))))
    }

    /// Paths the sandbox fills itself: the runtimes, its own /proc, /dev,
    /// private HOME and fresh /tmp.
    fn sandbox_paths() -> Vec<&'static Path> {
        RUNTIME_ROOTS
            .iter()
            .chain(&["/proc", "/dev", HOME, "/tmp"])
            .map(Path::new)
            .collect()
    }

    proptest! {
        /// The workspace bind must not replace a path the sandbox provides or sit inside one, except below the fresh /tmp.
        #[test]
        fn an_accepted_workspace_leaves_sandbox_paths_alone(root in absolute_path()) {
            if workspace_refusal(&root).is_none() {
                for provided in sandbox_paths() {
                    prop_assert!(!provided.starts_with(&root), "{} covers {}", root.display(), provided.display());
                    if provided != Path::new("/tmp") {
                        prop_assert!(!root.starts_with(provided), "{} is inside {}", root.display(), provided.display());
                    }
                }
            }
        }

        /// A read-only mount that settings accept must also pass the executor's check, or a valid config fails every command.
        #[test]
        fn settings_and_executor_agree_on_read_only_mounts(source in absolute_path(), home in absolute_path()) {
            if crate::settings::unsafe_mount(&source, Some(&home)).is_none() {
                prop_assert!(!overlaps_reserved(&source), "{}", source.display());
            }
        }
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
