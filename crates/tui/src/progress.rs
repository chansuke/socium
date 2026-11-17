use crate::app::App;
use crate::transcript::{CommandOutputState, Speaker};
use crate::view::text_layout::truncate_text;
use socium_core::AgentProgress;

impl App {
    pub(crate) fn apply_progress(&mut self, event: AgentProgress) {
        match event {
            AgentProgress::Activity(event) => {
                if let socium_core::ActivityKind::PlanUpdated { plan } = &event.kind {
                    self.timeline.show(Some(plan.clone()));
                }
                self.activity.apply(event);
            }
            AgentProgress::RawProviderEvent {
                turn_id,
                round,
                json,
            } => self.activity.push_raw(turn_id, round, json),
            AgentProgress::ToolCallRecorded(entry) => {
                self.pop_thinking_message();
                self.messages
                    .extend(crate::transcript::transcript_entries_to_chat_entries(vec![
                        entry,
                    ]));
                self.transcript_scroll = 0;
            }
            AgentProgress::CommandOutput {
                stdout_bytes,
                stderr_bytes,
            } => {
                self.status = Some(format!(
                    "Command running: stdout {stdout_bytes} bytes, stderr {stderr_bytes} bytes"
                ));
            }
            AgentProgress::ApprovalRequested {
                request_id,
                tool,
                summary,
                input,
                requested_by,
                session_option,
            } => {
                self.pop_thinking_message();
                let (summary, details) = match requested_by {
                    Some(origin) => (
                        format!("{origin}: {summary}"),
                        format!("Approve {tool}\nRequested by: {origin}\n{input}"),
                    ),
                    None => (summary, format!("Approve {tool}\n{input}")),
                };
                self.push_message(Speaker::System, details.clone());
                self.open_approval(request_id, tool, summary, details, session_option);
            }
            AgentProgress::ApprovalResolved { request_id, .. } => {
                self.close_unanswered_approval(request_id);
            }
            AgentProgress::ModelRequestStarted { round, provider } => {
                self.show_thinking_indicator();
                self.status = Some(model_request_status(round, &provider));
            }
            AgentProgress::ApprovalRequired { tool, reason } => {
                self.push_activity(format!("Approval required for {tool}: {reason}"));
                self.push_message(
                    Speaker::System,
                    format!("Approval required for {tool}: {reason}"),
                );
                self.status = Some("Waiting for approval".to_string());
            }
            AgentProgress::AssistantText { text } => {
                self.pop_thinking_message();
                if !text.is_empty() {
                    self.append_streamed_assistant_text(&text);
                    self.status = Some("Responding".to_string());
                }
            }
            AgentProgress::ThinkingText { text } => {
                if !text.is_empty() {
                    self.status = Some("Thinking".to_string());
                }
            }
            AgentProgress::ToolCallStarted { name } => {
                self.status = Some(format!("Tool: {name}"));
            }
            AgentProgress::ToolCallFinished { name, is_error } => {
                self.status = Some(if is_error {
                    format!("Tool failed: {name}")
                } else {
                    format!("Tool finished: {name}")
                });
            }
            AgentProgress::CommandStarted { command, .. } => {
                self.status = Some(format!("Command: {}", truncate_text(&command, 36)));
            }
            AgentProgress::CommandFinished {
                command,
                exit_code,
                timed_out,
                stdout_preview,
                stderr_preview,
                ..
            } => {
                self.status = Some(command_completion_status(&command, exit_code, timed_out));
                self.activity.attach_command_output(CommandOutputState {
                    command,
                    exit_code,
                    timed_out,
                    stdout_preview,
                    stderr_preview,
                });
            }
            AgentProgress::CompactionStarted => {
                self.push_message(
                    Speaker::System,
                    "Compacting conversation context…".to_string(),
                );
                self.status = Some("Compacting".to_string());
            }
            AgentProgress::CompactionFinished {
                covered_messages,
                used_llm,
            } => {
                let message = format_compaction_finished_message(covered_messages, used_llm);
                self.push_message(Speaker::System, message);
                self.status = Some("Compacted".to_string());
            }
        }
    }
}

fn format_compaction_finished_message(covered_messages: usize, used_llm: bool) -> String {
    let summarizer = if used_llm {
        "model summary"
    } else {
        "deterministic fallback"
    };
    format!("Compacted context: {covered_messages} messages summarized ({summarizer})")
}

fn model_request_status(round: usize, provider: &str) -> String {
    if round <= 1 {
        format!("Thinking ({provider})")
    } else {
        format!("Thinking ({provider}, round {round})")
    }
}

fn command_completion_status(command: &str, exit_code: Option<i32>, timed_out: bool) -> String {
    if timed_out {
        return format!("Command timed out: {}", truncate_text(command, 24));
    }
    match exit_code {
        Some(0) => format!("Command finished: {}", truncate_text(command, 24)),
        Some(code) => format!("Command failed ({code}): {}", truncate_text(command, 24)),
        None => format!("Command terminated: {}", truncate_text(command, 24)),
    }
}

#[cfg(test)]
mod tests {
    use super::{command_completion_status, model_request_status};

    #[test]
    fn model_request_status_includes_round_after_first_request() {
        assert_eq!(model_request_status(1, "anthropic"), "Thinking (anthropic)");
        assert_eq!(
            model_request_status(2, "anthropic"),
            "Thinking (anthropic, round 2)"
        );
    }

    #[test]
    fn command_completion_status_reports_failures_and_timeouts() {
        assert_eq!(
            command_completion_status("cargo test", Some(0), false),
            "Command finished: cargo test"
        );
        assert_eq!(
            command_completion_status("cargo test", Some(101), false),
            "Command failed (101): cargo test"
        );
        assert_eq!(
            command_completion_status("cargo test", None, true),
            "Command timed out: cargo test"
        );
    }

    #[test]
    fn approvals_from_a_subagent_say_which_one_asked() {
        let mut app = crate::app::App::for_test();
        app.apply_progress(socium_core::AgentProgress::ApprovalRequested {
            request_id: 7,
            tool: "run_command".into(),
            summary: "cargo test".into(),
            input: "Arguments: ...".into(),
            requested_by: Some("worker subagent \"Fix the test\"".into()),
            session_option: None,
        });

        let prompt = app.approval.as_ref().unwrap();
        assert_eq!(
            prompt.summary,
            "worker subagent \"Fix the test\": cargo test"
        );
        assert_eq!(
            prompt.details,
            "Approve run_command\nRequested by: worker subagent \"Fix the test\"\nArguments: ..."
        );
    }
}
