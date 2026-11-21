# Working on Socium

Read [docs/architecture.md](docs/architecture.md) for the crates, the layers
of `socium-core`, and the APIs with one entry point. Read
[docs/feature-map.md](docs/feature-map.md) for the names of features and
where each one lives. Use those names in commits and pull requests.

The checks hold the rules. `crates/core/tests/architecture.rs` checks the
layers, `clippy.toml` and `[workspace.lints]` cover the banned APIs and lint
allows, and `scripts/check-escape-hatches.sh` checks added lines. When one
fails, its message names the fix. Change the design to pass it. Do not
silence the check.

## Before you call a change done

- Run `scripts/verify`, then read `target/verify/summary.md`. Check each
  `docs/verification.md` item it lists.
- TUI change: run `scripts/verify tui` and read the frames. Add a scenario
  in `crates/tui/scenarios/` for new behavior.
- Tool or approval state change: run `scripts/verify diagrams`.
- Change to `flake.nix`, `Cargo.lock`, `*.example`, or tests that read files
  or start processes: run `scripts/verify full`.
- Commit inside `nix develop`, where pre-commit runs the same checks.

## When a bug is fixed or a review finds something

First ask whether a lint, a type, a test, the architecture check, or a
`docs/verification.md` item can stop it from coming back, and add that.
Write a rule here only when none of them can express it.
