//! Bounded command execution. Unix process groups cover ordinary descendants;
//! programs that deliberately detach require an OS sandbox, not this runner.
use crate::control::TaskControl;
use std::{
    io,
    path::Path,
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

pub(crate) struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub truncated: bool,
}

pub(crate) fn run(
    command: &[String],
    cwd: &Path,
    timeout: Duration,
    control: &TaskControl,
) -> io::Result<ProcessOutput> {
    run_observed(command, cwd, timeout, control, &mut |_, _| Ok(()))
}

#[cfg(unix)]
pub(crate) fn run_observed(
    command: &[String],
    cwd: &Path,
    timeout: Duration,
    control: &TaskControl,
    observer: &mut dyn FnMut(bool, &[u8]) -> io::Result<()>,
) -> io::Result<ProcessOutput> {
    use std::{
        io::Read,
        os::{fd::AsRawFd, unix::process::CommandExt},
    };
    const LIMIT: usize = 256 * 1024;
    fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
        // SAFETY: the pipe owns a live descriptor throughout both fcntl calls.
        let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn drain(
        pipe: &mut impl Read,
        bytes: &mut Vec<u8>,
        truncated: &mut bool,
        stderr: bool,
        observer: &mut dyn FnMut(bool, &[u8]) -> io::Result<()>,
    ) -> io::Result<bool> {
        let mut buf = [0; 8192];
        // Bound work per iteration as well as memory: an endless producer must
        // not prevent deadline or cancellation checks.
        for _ in 0..8 {
            match pipe.read(&mut buf) {
                Ok(0) => return Ok(true),
                Ok(n) => {
                    observer(stderr, &buf[..n])?;
                    let keep = n.min(LIMIT.saturating_sub(bytes.len()));
                    bytes.extend_from_slice(&buf[..keep]);
                    *truncated |= keep < n;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }
    struct ChildGuard(std::process::Child);
    impl ChildGuard {
        fn terminate(&mut self) {
            // SAFETY: spawn created a new process group whose ID is child.id().
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.kill();
        }
    }
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            self.terminate();
            let _ = self.0.wait();
        }
    }
    if control.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Command cancelled before spawn",
        ));
    }
    let program = command
        .first()
        .ok_or_else(|| io::Error::other("empty command"))?;
    let mut builder = Command::new(program);
    builder
        .args(&command[1..])
        .current_dir(cwd)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("DEEPSEEK_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if Path::new(program)
        .file_name()
        .is_some_and(|name| name == "git")
    {
        // Git hooks set these for their own repository/index. They must not
        // redirect a workspace tool (or a temporary-repository regression test).
        for name in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        ] {
            builder.env_remove(name);
        }
    }
    let mut child = ChildGuard(builder.spawn()?);
    let mut stdout = child.0.stdout.take().expect("piped stdout");
    let mut stderr = child.0.stderr.take().expect("piped stderr");
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let start = Instant::now();
    let mut stop = None;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut truncated = false;
    let mut timed_out = false;
    let mut cancelled = false;
    loop {
        let out_done = drain(&mut stdout, &mut out, &mut truncated, false, observer)?;
        let err_done = drain(&mut stderr, &mut err, &mut truncated, true, observer)?;
        let status = child.0.try_wait()?;
        if let Some(status) = status
            && out_done
            && err_done
        {
            return Ok(ProcessOutput {
                status,
                stdout: out,
                stderr: err,
                timed_out,
                cancelled,
                truncated,
            });
        }
        if stop.is_none() && (control.is_cancelled() || start.elapsed() >= timeout) {
            cancelled = control.is_cancelled();
            timed_out = !cancelled;
            child.terminate();
            stop = Some(Instant::now());
        }
        if stop.is_some_and(|at: Instant| at.elapsed() >= Duration::from_millis(200)) {
            let status = child.0.wait()?;
            return Ok(ProcessOutput {
                status,
                stdout: out,
                stderr: err,
                timed_out,
                cancelled,
                truncated: true,
            });
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(not(unix))]
pub(crate) fn run_observed(
    _: &[String],
    _: &Path,
    _: Duration,
    _: &TaskControl,
    _: &mut dyn FnMut(bool, &[u8]) -> io::Result<()>,
) -> io::Result<ProcessOutput> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "managed command execution currently requires Unix process groups",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn shell(script: &str, timeout: u64, control: &TaskControl) -> ProcessOutput {
        run(
            &["sh".into(), "-c".into(), script.into()],
            Path::new("/tmp"),
            Duration::from_millis(timeout),
            control,
        )
        .unwrap()
    }
    #[test]
    fn drains_both_pipes_with_bounded_memory() {
        let out = shell(
            "head -c 2097152 /dev/zero & head -c 2097152 /dev/zero >&2 & wait",
            5000,
            &TaskControl::default(),
        );
        assert!(out.status.success());
        assert!(!out.timed_out);
        assert!(out.truncated);
        assert_eq!(out.stdout.len(), 262144);
        assert_eq!(out.stderr.len(), 262144);
    }
    #[test]
    fn inherited_pipes_and_cancel_have_bounded_cleanup() {
        let start = Instant::now();
        let out = shell("sleep 30 & exit 0", 50, &TaskControl::default());
        assert!(out.timed_out);
        assert!(start.elapsed() < Duration::from_secs(2));
        let control = TaskControl::default();
        control.cancel();
        assert!(
            run(
                &["sh".into(), "-c".into(), "sleep 30".into()],
                Path::new("/tmp"),
                Duration::from_secs(30),
                &control
            )
            .is_err()
        );
    }
    #[test]
    fn signal_is_failure() {
        assert!(
            !shell("kill -TERM $$", 1000, &TaskControl::default())
                .status
                .success()
        );
    }
}
