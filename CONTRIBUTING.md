# Contributing

Socium is an early Rust coding agent. It speaks three wire formats: Anthropic
Messages (Anthropic and compatible endpoints such as DeepSeek and Z.ai), OpenAI
Chat Completions, and the OpenAI Responses API. Web search through DeepSeek is
off unless turned on in `config.toml`.

Use Linux with Rust, Git, ripgrep, bubblewrap 0.12+, and working unprivileged user
namespaces. The pinned Nix development shell supplies the compiler and native
dependencies used by CI. Sandbox enforcement tests fail if the backend is not
available; they must not be silently skipped or run without isolation:

```sh
nix develop
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
nix build .#default
```

Run the application from a checkout with
`nix develop -c cargo run --locked -p socium-tui`, or build and run the
packaged application with `nix run .`. `nix develop` provides the pinned
toolchain and bubblewrap 0.12.0; plain `cargo run` outside it uses the host
`bwrap`, which is refused when older than 0.12.0.

The workspace has two crates and an evaluation suite:

- `crates/core`: domain logic, providers, tool runtime, and input handling.
- `crates/tui`: Ratatui application entrypoint (binary crate).
- `evals/`: fixed tasks with hidden checks for comparing changes against a real
  model within a budget.

Tests use temporary repositories and scripted providers. Do not add paid API
calls or real credentials to ordinary tests. Run from the repository root.

Keep changes focused and explain the behavior, tests, and remaining limitations
in a pull request. Add regression tests for behavior and security fixes. Preserve
the user's staged, unstaged, and untracked changes in tool fixtures.

Planned work and its acceptance criteria are in GitHub Issues. Update current
documentation when behavior changes.

`nix develop -c pre-commit install` runs the checks above before each commit.
It also checks each commit message, because links to coding-agent sessions do
not belong in the history. The Commit metadata workflow checks the commits of
each pull request the same way.

Local sessions, `.env` files, Nix `result` links, and research checkouts must not
be committed. Before publishing, scan both the tracked tree and Git history for
secrets; deleting a credential from the latest tree does not remove it from Git.
Report security findings using [SECURITY.md](SECURITY.md), not public issues.
