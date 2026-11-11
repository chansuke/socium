# socium

A minimal AI agent for multiple platforms.

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
