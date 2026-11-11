# socium

A minimal AI agent for multiple platforms.

Our goal is a Rust coding agent that is easy to install and customize, works with
your preferred model, preserves your existing changes, and lets you stop and
resume work.

## Workspace layout

- `crates/core`: domain logic, providers, tool runtime, and input handling.
- `crates/tui`: Ratatui application entrypoint (binary crate).

## Run

```bash
cargo run -p socium-tui
```

Set `ANTHROPIC_API_KEY` before starting. The default model is Claude Opus 5.5
(`claude-opus-5-5`); set `MODEL` to override it at startup. `COMPACT_MODEL`
separately controls the summarization model, which defaults to `claude-haiku-4-5`.

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
