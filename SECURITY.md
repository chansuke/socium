# Security Policy

## Supported versions

Socium is pre-release software. Security fixes target the current `main` branch;
older commits are not maintained as separate release lines.

## Trust boundaries

- Use a trusted workspace. Workspace files and tool output are untrusted model
  input and may influence edits or proposed commands. Inspect requested commands
  and diffs. A human approval authorizes host-user execution, not a sandbox.
- File tools constrain paths to the workspace and protect `.git` and `.socium`.
  These checks do not isolate concurrently changing files, malicious host
  processes, or explicitly approved commands. OS isolation is tracked in #55.
- Files read by the agent and command output may be transmitted to Anthropic.
  `.gitignore` does not prevent explicit reads. Do not put credentials in the
  workspace or paste them into prompts. Model requests do not follow redirects.
- `.socium` contains conversation, input history, edit snapshots, and command
  output in plaintext. On Unix its directories are restricted to the current
  user; newly saved state/artifacts use mode 0600. Links and non-regular state
  files are refused. Back up and protect this directory; do not commit it.
- Ordinary Git tools refuse repositories with configured external content
  filters, disable external diff/text conversion and signature helpers, and do
  not inspect submodule worktrees. Use a separately approved host command when
  those features are needed. Git hooks can run during approved commits.
- Cancellation stops managed process groups; deliberately detached processes
  and operations with unknown crash outcomes require manual inspection.

See [recovery procedures](docs/daily-reliability.md) and the
[public release review](docs/public-release-review.md) for verification and limits.

## Reporting a vulnerability

**Do not report vulnerability details through public GitHub issues.**

Use [GitHub private vulnerability reporting](https://github.com/chansuke/socium/security/advisories/new).
Include the affected commit, a minimal reproduction using fake data, expected
behavior, and impact. Never attach real API keys, private prompts, or complete
session directories. If private reporting is unavailable, open a public issue
requesting a private contact method without disclosing the vulnerability.
