//! Bounded output capture and cancellation, always behind the OS executor.
use crate::control::TaskControl;
use crate::execution::{ExecutionPolicy, SandboxExecutor, WorkspaceAccess};
use std::{
    io,
    path::Path,
    process::{ExitStatus, Stdio},
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
    let policy = ExecutionPolicy::new(cwd, WorkspaceAccess::ReadOnly)?;
    run_observed(command, cwd, timeout, control, &mut |_, _| Ok(()), &policy)
}

#[cfg(unix)]
pub(crate) fn run_observed(
    command: &[String],
    cwd: &Path,
    timeout: Duration,
    control: &TaskControl,
    observer: &mut dyn FnMut(bool, &[u8]) -> io::Result<()>,
    policy: &ExecutionPolicy,
) -> io::Result<ProcessOutput> {
    use std::{
        io::Read,
        os::{fd::AsRawFd, unix::process::CommandExt},
    };
    const LIMIT: usize = 256 * 1024;
    // Errors and summaries come last, so a long stream keeps its start
    // and its newest output, and drops the middle.
    const HEAD: usize = 32 * 1024;
    const CUT: &[u8] =
        b"\n[... middle of the output omitted; the saved artifact has all of it ...]\n";
    fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
        // SAFETY: the pipe owns a live descriptor throughout both fcntl calls.
        let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: as above.
        if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
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
                    bytes.extend_from_slice(&buf[..n]);
                    if bytes.len() > LIMIT {
                        if !*truncated {
                            bytes.splice(HEAD..HEAD, CUT.iter().copied());
                            *truncated = true;
                        }
                        let excess = bytes.len() - LIMIT;
                        let from = HEAD + CUT.len();
                        bytes.drain(from..from + excess);
                    }
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
    let mut builder = SandboxExecutor::command(policy, command, cwd)?;
    builder
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
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
    let mut out_done = false;
    let mut err_done = false;
    loop {
        out_done = out_done || drain(&mut stdout, &mut out, &mut truncated, false, observer)?;
        err_done = err_done || drain(&mut stderr, &mut err, &mut truncated, true, observer)?;
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
        // Sleep until output, end of output, or the next cancellation and
        // deadline check, instead of polling on a fixed beat.
        let mut fds: Vec<libc::pollfd> = [
            (out_done, stdout.as_raw_fd()),
            (err_done, stderr.as_raw_fd()),
        ]
        .into_iter()
        .filter(|(done, _)| !done)
        .map(|(_, fd)| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
        if fds.is_empty() {
            // Both pipes closed but the process has not exited yet.
            std::thread::sleep(Duration::from_millis(1));
        } else {
            // SAFETY: `fds` holds live descriptors owned by the pipes above.
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 10) };
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn run_observed(
    _: &[String],
    _: &Path,
    _: Duration,
    _: &TaskControl,
    _: &mut dyn FnMut(bool, &[u8]) -> io::Result<()>,
    _: &ExecutionPolicy,
) -> io::Result<ProcessOutput> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "OS sandbox unavailable: command execution requires Linux and bubblewrap; no host fallback",
    ))
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    fn shell(script: &str, timeout: u64, control: &TaskControl) -> ProcessOutput {
        let workspace = tempfile::tempdir().unwrap();
        run(
            &["sh".into(), "-c".into(), script.into()],
            workspace.path(),
            Duration::from_millis(timeout),
            control,
        )
        .unwrap()
    }
    #[test]
    fn a_long_stream_keeps_its_start_and_its_end() {
        // Review N12: capture kept the first 256 KiB, so the failure at the
        // end of a long build log was lost.
        let out = shell(
            "echo first; head -c 1048576 /dev/zero | tr '\\0' x; echo; echo the last line",
            10_000,
            &TaskControl::default(),
        );
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.truncated);
        assert!(text.starts_with("first\n"));
        assert!(
            text.ends_with("the last line\n"),
            "{}",
            &text[text.len() - 100..]
        );
        assert!(text.contains("middle of the output omitted"));
        assert!(out.stdout.len() <= 256 * 1024);
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
        // The namespace reaper now kills descendants when the main command
        // exits, so inherited pipes close without waiting for the deadline.
        // Allow setup time; a 50ms timeout raced with successful sandbox exit.
        let out = shell("sleep 30 & exit 0", 1000, &TaskControl::default());
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out.timed_out);
        assert!(start.elapsed() < Duration::from_secs(2));
        let out = shell("sleep 30 & wait", 50, &TaskControl::default());
        assert!(out.timed_out);
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
