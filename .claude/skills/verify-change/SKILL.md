---
name: verify-change
description: Verify a code change with the repository's own verification entry point (scripts/verify in socium) and turn its artifacts — step logs, summary.md, TUI frames, state diagrams, checklist items — into a short report that separates what was confirmed from what was not run. Use when finishing a change, before opening or updating a PR, when the user asks to verify, check, or prove a change works, or asks for verification artifacts or a verification summary. Not for writing the tests themselves.
---

# Verify a change

Run the repository's deterministic checks, read what they produced, and
report. Do not judge a change by reading its diff alone.

## 1. Find the entry point

- `scripts/verify` exists: use it (socium). Read its header for the modes.
- Otherwise use what AGENTS.md, CLAUDE.md, or CONTRIBUTING.md name. If none
  is named, say so and run the project's formatter, linter, and tests.

## 2. Choose the modes from the diff

`git diff --name-only <base>...HEAD` plus uncommitted files decide:

| Changed | Run |
|---|---|
| anything | `scripts/verify` |
| `crates/tui/` | also `scripts/verify tui`, then read the frames of the scenarios the change touches |
| `crates/tui/src/activity.rs`, tool or approval states | also `scripts/verify diagrams`, and read the `.mmd` files |
| `crates/tui/src/headless.rs`, `cli.rs`, `-p` output | also `scripts/verify headless` |
| `flake.nix`, `Cargo.lock`, `*.example`, tests that read files or start processes | `scripts/verify full` (the Nix build sandbox) |

Run modes one at a time; each overwrites `target/verify/summary.md`, so read
it after each run.

## 3. Read the artifacts, not only the exit code

- `target/verify/summary.md`: the step table and the checklist items the
  change triggers. Each checklist item needs its own check or a stated
  reason it does not apply.
- A failed step: read its log in `target/verify/logs/`. Fix the cause. Do
  not weaken an assertion, add `#[expect]` or a sleep, or re-export to get
  past the architecture test; the error message names the intended fix.
- `target/verify/tui/*.txt`: confirm the frames show the behavior the change
  claims, not just that the scenario passed. A new UI behavior gets a new
  scenario in `crates/tui/scenarios/` and a row in `docs/feature-map.md`.
- `target/verify/diagrams/*.mmd`: confirm the drawn transitions match the
  intended design.

## 4. Before reporting, ask what would stop a recurrence

For a bug fix or a review finding, check whether a lint (`clippy.toml`,
`[workspace.lints]`), a type, a test, the architecture check, or a
`docs/verification.md` item can catch it next time. Add that instead of a
prose rule, or say why none can.

## 5. Report

In the user's language, short:

1. Confirmed: each mode run, pass or fail, with the summary path.
2. Evidence: the frames, diagrams, or log lines that show the behavior,
   quoted briefly with their paths.
3. Checklist items: each one triggered and how it was checked.
4. Not verified: modes not run and why, and anything only a real provider or
   a person can check.

For a PR body, keep the same split in English, one to three plain
sentences per part.
