# Feature map

Each row is a feature as a user names it. Next to it are how the user
reaches it, where the code is, and what checks it. Use these names in
issues, pull requests, and reviews. `scripts/check-feature-map.sh`, which
`scripts/verify` runs, fails when a path or test named here no longer
exists. Run `scripts/verify tui` to see the frames of the TUI scenarios and
`scripts/verify diagrams` for the state diagrams of tool rows, and
read [Using Socium](usage.md) for how each feature behaves.

| Feature | How a user reaches it | Code | Checked by |
|---|---|---|---|
| Ask and get an answer | Type in the composer and press Enter | `crates/tui/src/app.rs`, `crates/tui/src/agent_task.rs`, `crates/core/src/agent/turn.rs`, `crates/core/src/providers/` | `a_turn_prints_only_versioned_json_lines_and_its_result` |
| Composer editing | Typing, Shift+Enter or Alt+Enter for a new line, Up and Down for history, Esc and E | `crates/tui/src/app.rs`, `crates/tui/src/input_history.rs` | `crates/tui/scenarios/composer-modes.toml`, `crates/tui/scenarios/multiline-input.toml`, `crates/tui/scenarios/wide-text.toml` |
| Slash commands | `/` in the composer | `crates/tui/src/slash_command.rs` | `crates/tui/scenarios/slash-commands.toml` |
| Approvals | A prompt before a command, a commit, or a sensitive edit or read: allow once, allow for the session, or deny | `crates/core/src/approval.rs`, `crates/core/src/access.rs`, `crates/core/src/control.rs`, `crates/core/src/dispatch.rs`, `crates/tui/src/approval.rs` | `approvals_follow_the_model_of_answers_grants_and_cancellation`, `a_session_answer_grants_what_was_offered_and_nothing_else`, `arrows_move_the_choice_and_enter_confirms_it`, `a_call_that_needs_approval_is_refused_and_the_model_is_told_why` |
| Sandboxed commands | The model runs `run_command` or `git_commit` | `crates/core/src/execution.rs`, `crates/core/src/execution/linux.rs`, `crates/core/src/process.rs`, `crates/core/src/tools/run_command.rs` | `policy_denies_outside_reads_writes_symlinks_and_wrappers`, `network_and_kernel_escape_interfaces_are_denied` |
| File edits and undo | The model edits files; `/undo` reverts the newest edit | `crates/core/src/tools/apply_patch.rs`, `crates/core/src/tools/text_editor.rs`, `crates/core/src/tools/edit_history.rs` | `undoing_every_edit_restores_the_files_as_they_were`, `undo_last_reverts_the_newest_edit_whatever_its_files`, `move_and_its_undo_keep_the_executable_mode` |
| Activity pane | Ctrl+A to focus, Up and Down, Enter to expand, r for raw events, z for full screen | `crates/core/src/activity.rs`, `crates/core/src/activity/signals.rs`, `crates/tui/src/activity.rs`, `crates/tui/src/view/activity.rs` | `crates/tui/scenarios/activity-pane.toml`, `tool_rows_move_only_as_the_transition_table_says` |
| Plan and Timeline | The model keeps a plan with `update_plan`; Ctrl+T shows it | `crates/core/src/plan.rs`, `crates/tui/src/timeline.rs`, `crates/tui/src/view/timeline.rs` | `a_new_plan_lists_tasks_in_tree_order_with_groups_derived`, `it_follows_the_active_task_until_a_task_is_selected` |
| Subagents | The model calls the `agent` tool; `/agents` lists them | `crates/core/src/agent/subagent/`, `crates/tui/src/subagents.rs` | `explorer_works_in_a_fresh_context_and_only_its_report_comes_back`, `worker_gets_every_tool_but_agent` |
| Sessions | `socium -c`, `socium -r`, `/resume`, `/rename`, `/clear` | `crates/core/src/session.rs`, `crates/tui/src/session_picker.rs` | `the_legacy_conversation_and_its_activity_move_into_a_session`, `rows_show_age_count_and_preview_and_scroll_to_the_selection` |
| Rewind | `/rewind` to go back to an earlier prompt | `crates/core/src/lib.rs`, `crates/tui/src/rewind_picker.rs` | `closing_the_picker_keeps_the_conversation` |
| Model choice | `/model`, and Tab to complete an alias | `crates/core/src/models.rs`, `crates/tui/src/model_picker.rs` | `the_picker_opens_on_the_active_model_and_moves_within_bounds` |
| Compaction | `/compact`, or automatically when the context fills | `crates/core/src/agent/compaction.rs` | `compaction_threshold_uses_the_lowest_of_model_default_and_ratio` |
| Instruction files | `AGENTS.md` in the workspace | `crates/core/src/instructions.rs` | `agents_md_and_exposed_directories_are_added` |
| Web search | Turned on in `config.toml` | `crates/core/src/tools/web_search.rs` | `results_come_back_with_their_sources_and_the_query_as_written` |
| Token use and cost | The footer | `crates/core/src/usage.rs` | `rounds_add_up_per_model_including_subagents` |
| `socium -p` | One turn without the TUI, as text or JSON lines | `crates/tui/src/cli.rs`, `crates/tui/src/headless.rs` | `an_interrupt_cancels_the_turn_and_still_reports_it` |
| Configuration | `config.toml` and `models.toml` | `crates/core/src/settings.rs`, `crates/core/src/models.rs` | `the_example_file_is_valid` |
