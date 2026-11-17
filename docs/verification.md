# Verification checklist

Each item below comes from bugs this repository already shipped and fixed.
An item has a trigger you can read from a diff, a check to run or write, and
the commits behind it. Check only the items whose trigger matches your change.

```sh
scripts/verification-checklist.sh             # changes since main, uncommitted ones included
scripts/verification-checklist.sh HEAD~3      # changes since another revision
scripts/verification-checklist.sh A~1 A       # the changes of commit A
```

The script reads the `Paths` and `Added lines` of each item. A path is a shell
pattern in which `*` also matches `/`. An added-lines pattern is an extended
regular expression matched against the lines the diff adds.

Before a pull request, run the cheap checks first: `cargo fmt`, `cargo clippy`
and `cargo test` (the pre-commit hook runs all three), then the items here.

## Adding and removing items

Add an item only when the same mistake reached at least two commits or pull
requests and a diff shows when it applies. Cite the commits. Remove an item
when its code is gone or a test now catches the mistake on its own.

## Items

### Live and replayed views agree

- Paths: `crates/core/src/activity*`, `crates/core/src/transcript.rs`, `crates/tui/src/activity.rs`, `crates/tui/src/transcript.rs`, `crates/tui/src/rewind_picker.rs`, `crates/core/src/agent/subagent/*`
- Check: What a turn shows while it runs must equal what a restart, `-r`, or
  `/rewind` rebuilds from disk. Compare both in a test, as
  `the_live_conversation_shows_agent_calls_as_a_restart_does` does.
- Incidents: `8848f65` (#50), `36c633b` (#52), `8de4b46`.

### The subagent path gets the main agent's change

- Paths: `crates/core/src/lib.rs`, `crates/core/src/agent/turn.rs`, `crates/core/src/agent/compaction.rs`, `crates/core/src/agent/subagent/*`
- Check: A change to how the main agent compacts, records approvals, frames
  instructions, or warns at startup usually applies to subagents too. Make
  the same change in `agent/subagent/` or say in the PR why it does not apply.
- Incidents: `c2e4e9d` (#54), `868198d` (#53), `7788f21` (#53).

### Tests pass in the Nix build sandbox

- Paths: `flake.nix`, `crates/tui/tests/*`, `*.example`
- Added lines: `include_str!|include_bytes!|Command::new\("git"\)`
- Check: Run `nix build .#default`. Its check phase has no network, no home
  directory, and only the files in the flake's source set, so a test that
  reads a file outside `crates/`, starts Git or the binary, or needs CA
  certificates can pass under `cargo test` and still fail there.
- Incidents: `0ec5d4d`, `ff82169`, `9e18704`.

### Terminal text is measured in cells

- Paths: `crates/tui/src/view/*`, `crates/tui/src/*picker.rs`
- Check: Width, wrapping, truncation, and cursor placement use the helpers in
  `crates/tui/src/view/cells.rs`, not byte or `char` counts. Test with
  Japanese text and wide symbols.
- Incidents: `9ac14ef` (#101), `d4a5a10` (#104), `2dbfaff`.

### Edits keep the file as it was outside the change

- Paths: `crates/core/src/tools/apply_patch.rs`, `crates/core/src/tools/text_editor.rs`, `crates/core/src/tools/text_file.rs`, `crates/core/src/tools/edit_history.rs`, `crates/core/src/tools/git.rs`
- Check: Line endings, permission bits, symlinks, and special files survive
  every write, move, undo, and commit. Test each kind the change touches,
  as `untouched_lines_keep_their_own_line_endings` and
  `move_and_its_undo_keep_the_executable_mode` do.
- Incidents: `9c0e1a8`, `a8597ba`, `93aa9b9`, `284e219`.

### Thinking blocks go back unchanged, and only where they are valid

- Paths: `crates/core/src/agent/types.rs`, `crates/core/src/agent/context.rs`, `crates/core/src/agent/compaction.rs`, `crates/core/src/providers/*`
- Check: `thinking` and `redacted_thinking` blocks are sent back byte for byte
  to the model that wrote them, and dropped where a summary cut off their
  prefix or the provider cannot read them (ADR 14).
- Incidents: `3c2f3ba`, `354c485`.

### Long output keeps its end

- Paths: `crates/core/src/process.rs`, `crates/core/src/tools/run_command.rs`, `crates/core/src/tools/search_text.rs`, `crates/core/src/tools/ripgrep.rs`, `crates/core/src/tools/list_files.rs`, `crates/core/src/tools/read_file.rs`, `crates/core/src/agent/context.rs`, `crates/core/src/constants.rs`
- Check: Every cut is bounded and says it cut. Command output keeps its end,
  where failures print, and one long line cannot fill the result. The
  `tail-failure` eval task measures this with a real model.
- Incidents: `21567b9`, `c284e9d`, `0c5e452`.

### Provider streams fail closed and keep what arrived

- Paths: `crates/core/src/providers/*`
- Check: Unknown events are skipped, a reply cut at the output limit is kept
  and reported, malformed tool input is reported to the model and never
  runs, and an error shows the provider's type and message.
- Incidents: `2e4946f`, `d4a5a10` (#104), `bf1f9bb` (#102).
