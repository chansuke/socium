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
packaged application, use `nix run .`. Linux is tested in CI; macOS is a declared
Nix target but has not been validated. Windows command execution is unsupported.

Set `ANTHROPIC_API_KEY` before starting. The default model is Claude Opus 5.5
(`claude-opus-5-5`); set `MODEL` to override it at startup. `COMPACT_MODEL`
separately controls the summarization model, which defaults to `claude-haiku-4-5`.
Anthropic is currently the only supported provider. Other providers and Web
search are planned. Model names must be available to your API account.

Install Git and ripgrep (`rg`) for repository tools. The Nix package includes both
on its runtime PATH. Managed command execution currently requires Unix.

## Everyday work

- Read and search workspace files, preview patches, and undo recorded edits.
- Review host commands and commit diffs in the TUI; press `y` to allow once or `n`
  to deny. Commands run with your user permissions, without an OS sandbox.
- Press Esc or Ctrl+C during a task to cancel and wait for cleanup.
- Restart in the same workspace to load saved conversation and tool results.
  Interrupted operations with no saved result are marked unknown, never replayed.

Add `.socium/` to your project's `.gitignore`. See the
[reliability and recovery guide](docs/daily-reliability.md) for storage limits,
editing guarantees, migration, and recovery procedures.

Only run Socium in workspaces you trust. Read files and command output can be
sent to the model provider; `.gitignore` is not a confidentiality boundary.
Conversation, input history, edits, and command artifacts are stored locally
under `.socium/` without encryption. Keep API keys outside source files.

See [documentation](docs/index.md), [contributing](CONTRIBUTING.md), and the
[security policy](SECURITY.md). Licensed under [MIT](LICENSE).
