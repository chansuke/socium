# socium

A Rust coding agent for the terminal.

Our goal is a Rust coding agent that is easy to install and customize, works with
your preferred model, preserves your existing changes, and lets you stop and
resume work.

## Workspace layout

- `crates/core`: domain logic, providers, tool runtime, and input handling.
- `crates/tui`: Ratatui application entrypoint (binary crate).

## Run

```bash
cargo run --locked -p socium-tui
```

For the pinned development environment, use `nix develop`. To build and run the
packaged application, use `nix run .`. Execution currently requires Linux
(x86_64 or aarch64), bubblewrap, and unprivileged user namespaces. The TUI checks
the sandbox at startup and refuses to run if isolation cannot be established.
macOS and Windows execution are unsupported; there is no unsandboxed fallback.

Set `ANTHROPIC_API_KEY` before starting. The default model is Claude Opus 5.5
(`claude-opus-5-5`); set `MODEL` to override it at startup. `COMPACT_MODEL`
separately controls the summarization model, which defaults to `claude-haiku-4-5`.
Anthropic is currently the only supported provider. Other providers and Web
search are planned. Model names must be available to your API account.

Install Git, ripgrep (`rg`), and bubblewrap (`bwrap`, version 0.12 or newer).
The Linux Nix package includes them on its runtime PATH. Use a project directory
with an ordinary `.git` directory; linked worktrees and redirected Git metadata
are not yet supported.

## Everyday work

- Read and search workspace files, preview patches, and undo recorded edits.
- Review sandboxed commands and commit diffs in the TUI; press `y` to allow once
  or `n` to deny. Commands can write workspace files, but cannot access the host
  home or network. Existing Git metadata is read-only except during `git_commit`;
  Socium's state is hidden from all child processes.
- Press Esc or Ctrl+C during a task to cancel and wait for cleanup.
- Restart in the same workspace to load saved conversation and tool results.
  Interrupted operations with no saved result are marked unknown, never replayed.

Add `.socium/` to your project's `.gitignore`. See the
[Linux sandbox guide](docs/linux-sandbox.md) for the exact filesystem policy,
environment, toolchain requirements, and restrictions on networked builds.
See the
[reliability and recovery guide](docs/daily-reliability.md) for storage limits,
editing guarantees, migration, and recovery procedures.

Only run Socium in workspaces you trust. Read files and command output can be
sent to the model provider; `.gitignore` is not a confidentiality boundary.
Conversation, input history, edits, and command artifacts are stored locally
under `.socium/` without encryption. Keep API keys outside source files.

See [documentation](docs/index.md), [contributing](CONTRIBUTING.md), and the
[security policy](SECURITY.md). Licensed under [MIT](LICENSE).
