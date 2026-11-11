# Contributing

Socium is an early Rust coding agent. Anthropic is currently the only implemented
provider; other providers and Web search are tracked work, not available features.

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

Tests use temporary repositories and scripted providers. Do not add paid API
calls or real credentials to ordinary tests. Run from the repository root.

Keep changes focused and explain the behavior, tests, and remaining limitations
in a pull request. Add regression tests for behavior and security fixes. Preserve
the user's staged, unstaged, and untracked changes in tool fixtures.

Use the [ticket index](docs/tickets/README.md) for acceptance criteria and
dependencies. GitHub issue state is authoritative; historical reviews are not a
second backlog. Update current documentation when behavior changes.

Local sessions, `.env` files, Nix `result` links, and research checkouts must not
be committed. Before publishing, scan both the tracked tree and Git history for
secrets; deleting a credential from the latest tree does not remove it from Git.
Report security findings using [SECURITY.md](SECURITY.md), not public issues.
