# Threat model

What Socium protects, from whom, and what it leaves to you. The
[security policy](../SECURITY.md) states the same boundaries briefly; the
[Linux sandbox guide](linux-sandbox.md) gives the exact policy.

## What is trusted

- **You**, and what you type and approve.
- **Your configuration outside the workspace**: `~/.config/socium/config.toml`,
  `models.toml`, and the subagent definitions. Socium refuses to start in a
  workspace that contains them, so neither the model's file tools nor its
  commands can change them.
- **The host**: the kernel, the Socium binary, and the bubblewrap it runs.

## What is not trusted

- **The model's output.** It can be wrong, and it follows instructions it
  reads, including instructions planted in files, command output, or
  `AGENTS.md`. Nothing the model writes grants authority: a tool call cannot
  ask for a wider sandbox, approve itself, or choose its own permissions.
- **The workspace contents.** A cloned repository may be hostile. Its files
  are data to the model; its build scripts and Git configuration do not run on
  the host because Socium reads or edits them.
- **The model provider** sees everything sent to it: your prompts and every
  file, diff, and command output a tool returns. Register only endpoints you
  would show that content to.

## What is protected, and how

| Asset | Threat | Control |
|---|---|---|
| Host home and credentials (`~/.ssh`, cloud and Git credentials, keyrings) | A command reads them and the model sends them on | Commands run in bubblewrap with a private HOME and only the workspace, system runtimes, and the directories you list in `sandbox.read_only`. The home directory itself cannot be listed. |
| Provider API keys | A command or the model reads them | Keys stay in the parent process. Children get a fixed environment, never inherited variables. |
| The network | A command exfiltrates data or fetches code | Network namespace and seccomp deny all sockets except connected stream and seqpacket pairs, which reach only their own peer. Datagram pairs, which could reach host pathname sockets, are denied. |
| The host itself | An edit makes the host run code later: direnv loads `.envrc`, rust-analyzer runs `build.rs` and proc macros, CI and hook managers run their files | Edits to these files always ask for approval, with the diff (`approval.protected`). Commands nobody reviewed run with them read-only. |
| Secrets in the workspace (`.env`, keys) | The model reads them and they reach the provider | Reads by file tools always ask (`approval.sensitive`). Commands nobody reviewed see them as empty files. An approved command can still read them. |
| Git history and metadata | A command rewrites history, hooks, or config | `.git` is read-only except during `git_commit`, which always asks and shows the diff, the new files, and the author. A linked worktree's main Git directory is mounted only after its back links are checked. |
| Socium's own state (`.socium/`) | A command forges history, approvals, or settings | Hidden from every child process. |
| Your uncommitted work | The model overwrites or loses it | Edits are checked against the file's current content, recorded before they are written, and can be undone with `/undo`. Commits cover only the paths named in the request. |

## What approval is for

Approval exists for actions whose effect reaches beyond the sandbox or is hard
to undo: commands (by default), commits, edits to files the host runs, and
reads of secrets. It is not a substitute for the sandbox: an approved command
runs with the same isolation, and no answer widens it.

`approval.commands = "auto"` and `approval.allow` trade a prompt for a narrower
sandbox, not for trust. Choose them for commands whose worst case inside the
workspace you accept, such as `cargo test`.

## What Socium does not protect against

- A command you approve damaging workspace files it may write, or reading a
  secret file you let it read.
- Host tools that run workspace files not on the protected list. Add them to
  `approval.protected`.
- Instructions planted in the workspace that persuade the model to propose
  harmful but sandboxed work. Read approval requests before answering.
- Hardlinks into the workspace, a compromised kernel, hostile processes running
  as your user, and resource exhaustion. This is not a VM.
- What the provider does with the data it receives.
