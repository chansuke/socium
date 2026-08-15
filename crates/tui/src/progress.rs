use crate::app::App;
use crate::transcript::{CommandOutputState, Speaker};
use crate::view::text_layout::truncate_text;
use socium_core::AgentProgress;

impl App {
    pub(crate) fn apply_progress(&mut self, event: AgentProgress) {
        match event {
            AgentProgress::ModelRequestStarted { round, provider } => {
                self.push_activity(format!("Model request round {round} via {provider}"));
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
                    self.append_streamed_thinking_text(&text);
                    self.status = Some("Thinking".to_string());
                }
            }
            AgentProgress::ToolCallStarted { name } => {
                self.push_activity(format!("Running tool: {name}"));
                self.pop_thinking_message();
                self.push_message(Speaker::System, format!("Running tool: {name}"));
                self.status = Some(format!("Tool: {name}"));
            }
            AgentProgress::ToolCallFinished { name, is_error } => {
                self.push_activity(format!(
                    "Finished tool: {name}{}",
                    progress_suffix(is_error)
                ));
                self.push_message(
                    Speaker::System,
                    format!("Finished tool: {name}{}", progress_suffix(is_error)),
                );
                self.status = Some(if is_error {
                    format!("Tool failed: {name}")
                } else {
                    format!("Tool finished: {name}")
                });
            }
            AgentProgress::CommandStarted { command, cwd } => {
                self.push_activity(format!("Running command in {cwd}: {command}"));
                self.push_message(
                    Speaker::System,
                    format!("Running command in {cwd}: {command}"),
                );
                self.status = Some(format!("Command: {}", truncate_text(&command, 36)));
            }
            AgentProgress::CommandFinished {
                command,
                exit_code,
                timed_out,
                stdout_preview,
                stderr_preview,
            } => {
                self.push_activity(format_command_progress_message(
                    &command, exit_code, timed_out,
                ));
                self.last_command_output = Some(CommandOutputState {
                    command: command.clone(),
                    exit_code,
                    timed_out,
                    stdout_preview,
                    stderr_preview,
                });
                self.push_message(
                    Speaker::System,
                    format_command_progress_message(&command, exit_code, timed_out),
                );
                self.status = Some(command_completion_status(&command, exit_code, timed_out));
            }
            AgentProgress::CompactionStarted => {
                self.push_activity("Compacting conversation context".to_string());
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
                self.push_activity(message.clone());
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

fn progress_suffix(is_error: bool) -> &'static str {
    if is_error { " (error)" } else { "" }
}

fn format_command_progress_message(
    command: &str,
    exit_code: Option<i32>,
    timed_out: bool,
) -> String {
    if timed_out {
        return format!("Command timed out: {command}");
    }
    match exit_code {
        Some(code) => format!("Command finished ({code}): {command}"),
        None => format!("Command finished (terminated): {command}"),
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
    use super::{command_completion_status, format_command_progress_message, model_request_status};

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
    fn format_command_progress_message_reports_exit_code() {
        assert_eq!(
            format_command_progress_message("cargo test", Some(0), false),
            "Command finished (0): cargo test"
        );
        assert_eq!(
            format_command_progress_message("cargo test", None, true),
            "Command timed out: cargo test"
        );
    }
}
