# Observations of 2026-11-11 and the tests that now guard them

The fifteen observation tests in `review_repros.rs` and `stream_repros.rs` passed while each defect existed. They were run against a temporary copy and are not part of the build, so CI never runs them as safety tests. Each defect was fixed under the ticket below; the listed tests assert the safe behavior and run in the normal suite.

| Observation | Ticket | Regression tests |
|---|---|---|
| `review_model_supplied_approval_is_sufficient` | SOC-001 | `tools::run_command::tests::forged_approval_is_denied_and_human_approval_is_single_use`, `reliability_tests::declined_and_cancelled_approval_cannot_run_commands` |
| `review_new_path_dotdot_escapes_workspace` | SOC-002 | `tools::workspace::tests::resolve_for_create_rejects_escape_path`, `create_normalizes_missing_components_without_side_effects`, `resolve_for_create_rejects_dangling_symlink_path` |
| `review_safe_read_only_sed_can_write_without_approval` | SOC-003 | `execution::tests::policy_denies_outside_reads_writes_symlinks_and_wrappers`, `hidden_files_read_empty_and_frozen_paths_refuse_writes` |
| `review_command_reads_outside_workspace` | SOC-003 | `execution::tests::policy_denies_outside_reads_writes_symlinks_and_wrappers`, `state_is_hidden_and_git_is_readonly_except_for_commit_policy` |
| `review_pipe_output_causes_false_timeout` | SOC-004 | `process::tests::drains_both_pipes_with_bounded_memory` |
| `review_child_inherited_pipe_outlives_timeout` | SOC-004 | `process::tests::inherited_pipes_and_cancel_have_bounded_cleanup`, `execution::tests::detached_descendants_die_on_timeout` |
| `review_signal_death_reported_as_success` | SOC-004 | `process::tests::signal_is_failure` |
| `review_second_request_failure_loses_record_of_completed_edit` | SOC-005 | `reliability_tests::round_limit_keeps_all_completed_results`, `failed_result_save_recovers_unknown_without_repeating_edit`, `read_search_edit_test_failure_restart_and_resume_preserve_human_changes` |
| `review_emergency_context_cut_orphans_tool_result` | SOC-006 | `agent::context::tests::trimming_keeps_tool_pairs_and_current_user_input`; every scripted request is also checked by `test_support::check_request` |
| `review_small_history_is_summarized_and_starts_with_system` | SOC-006 | `agent::context::tests::short_history_that_fits_is_not_summarized`, `prepare_context_adds_summary_when_budget_is_tight`; `check_request` refuses a request that does not start with a user message |
| `review_incomplete_stream_can_finish_with_executable_tool` | SOC-007 | `providers::anthropic::stream::tests::eof_and_token_limit_never_release_tool_calls`, `providers::openai_chat::tests::a_stream_cut_before_its_finish_reason_fails` |
| `review_token_limit_is_discarded_by_stream_builder` | SOC-007 | `providers::anthropic::stream::tests::a_text_reply_cut_at_the_limit_is_kept_and_marked`, `providers::openai_responses::stream::tests::a_reply_cut_at_the_output_limit_is_kept_and_marked` |
| `review_dry_run_accepts_missing_update_and_unchecked_destination` | SOC-008 | `tools::apply_patch::tests::invalid_later_action_and_dry_run_have_no_side_effects` |
| `review_partial_patch_keeps_first_write_after_second_fails` | SOC-008 | `tools::apply_patch::tests::invalid_later_action_and_dry_run_have_no_side_effects` |
| `review_git_commit_includes_unrelated_unstaged_files` | SOC-010 | `tools::git::tests::scoped_commit_preserves_unrelated_index_and_untracked_files`, `reliability_tests::staged_only_commit_and_changed_approval_scope` |
