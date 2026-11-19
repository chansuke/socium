# Security Policy

## Supported versions

Socium is pre-release software. Security fixes target the current `main` branch;
older commits are not maintained as separate release lines.

## Trust boundaries

The [threat model](docs/threat-model.md) explains these boundaries and what
they leave to you.

- Use a trusted workspace. Workspace files, `AGENTS.md`, and tool output are
  untrusted model input and may influence edits or proposed commands. Read each
  approval request: it shows the command, the commit diff with new files and
  author, or the edit's diff. An answer allows the call once or, when offered,
  a command prefix or file for the rest of the session; it never widens the OS
  sandbox. Edits to files host tools run on their own (`build.rs`, `.envrc`,
  CI, editor settings) and reads of credential files always ask; wider
  searches and diffs leave those files out. Socium's
  settings must live outside the workspace, so the model cannot change them.
- File tools constrain paths to the workspace and protect `.git` and `.socium`.
  These checks do not isolate concurrently changing files, malicious host
  processes. All subprocesses use Linux bubblewrap with filesystem, PID, user,
  and network namespaces, plus a seccomp policy. Read-only helpers cannot write
  workspace files; approved commands can. Existing root Git metadata is writable
  only for `git_commit`, and `.socium` is hidden from every child process.
- Children receive a fixed environment allowlist and a private temporary HOME.
  No provider keys, host credential directories, proxy variables or agent sockets
  are inherited. Public system runtimes, and the directories you list in
  `sandbox.read_only`, are mounted read-only; other host paths are not exposed.
  Socket creation (including Unix sockets) and io_uring are denied; only
  connected stream and seqpacket socket pairs, which reach nothing but their own
  peer, are allowed. Model API calls remain in the parent process with provider
  credentials.
- The sandbox is required, not best effort. Initialization failures, unsupported
  platforms, and redirected Git metadata are refused; a linked worktree is
  accepted only when its Git links check out. It does
  not prevent erroneous edits to granted workspace files, resource exhaustion,
  kernel vulnerabilities, or hostile concurrent host processes. Pre-existing
  hardlinks share inodes: never link sensitive host files into a workspace.
- Files read by the agent and command output may be transmitted to the
  provider of the model in use, which can be any endpoint you registered.
  `.gitignore` does not prevent explicit reads. Do not put credentials in the
  workspace or paste them into prompts. Model requests do not follow redirects.
- `.socium` contains conversation, input history, edit snapshots, and command
  output in plaintext. On Unix its directories are restricted to the current
  user; newly saved state/artifacts use mode 0600. Links and non-regular state
  files are refused. Back up and protect this directory; do not commit it.
- Ordinary Git tools refuse repositories with configured external content
  filters, disable external diff/text conversion and signature helpers, and do
  not inspect submodule worktrees. Global/system Git configuration is not loaded
  inside the sandbox; when the repository sets no identity, `git_commit` passes
  your global `user.name` and `user.email` as the author, read by the host's Git
  outside any repository. Git hooks can run during approved commits, within the
  same filesystem, environment and network restrictions.
- Cancellation stops the sandbox and its PID namespace, including detached
  descendants. Operations with unknown crash outcomes still require inspection;
  isolation does not roll back file changes already made.

See [recovery procedures](docs/daily-reliability.md) and the
[Linux sandbox guide](docs/linux-sandbox.md) for verification and limits.

## Reporting a vulnerability

**Do not report vulnerability details through public GitHub issues.**

Use [GitHub private vulnerability reporting](https://github.com/chansuke/socium/security/advisories/new).
Include the affected commit, a minimal reproduction using fake data, expected
behavior, and impact. Never attach real API keys, private prompts, or complete
session directories. If private reporting is unavailable, open a public issue
requesting a private contact method without disclosing the vulnerability.
