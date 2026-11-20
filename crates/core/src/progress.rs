//! What a running turn reports to the UI as it goes.
use crate::activity::ActivityEvent;
use crate::transcript::TranscriptEntry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProgress {
    ModelRequestStarted {
        round: usize,
        provider: String,
    },
    ApprovalRequested {
        request_id: u64,
        tool: String,
        /// The tool's main argument on one line, such as the command.
        summary: String,
        input: String,
        /// The subagent that asked, such as `worker subagent "Fix the
        /// test"`; `None` when the main agent asked.
        requested_by: Option<String>,
        /// The "allow for this session" choice to offer, when there is one.
        session_option: Option<String>,
    },
    ApprovalRequired {
        tool: String,
        reason: String,
    },
    /// The human answered, the request timed out, or the task was cancelled.
    ApprovalResolved {
        request_id: u64,
        approved: bool,
    },
    /// Structured record of the turn for the Activity pane.
    Activity(ActivityEvent),
    /// One provider stream event as received. Shown on request, never saved.
    RawProviderEvent {
        turn_id: usize,
        round: usize,
        json: String,
    },
    AssistantText {
        text: String,
    },
    ThinkingText {
        text: String,
    },
    ToolCallStarted {
        name: String,
    },
    ToolCallFinished {
        name: String,
        is_error: bool,
    },
    /// A finished tool call as the transcript shows it after a restart.
    ToolCallRecorded(TranscriptEntry),
    CommandOutput {
        stdout_bytes: u64,
        stderr_bytes: u64,
    },
    CommandStarted {
        command: String,
        cwd: String,
    },
    CommandFinished {
        command: String,
        exit_code: Option<i32>,
        timed_out: bool,
        /// Whether either stream was cut in the result; the saved
        /// artifacts hold what was left out.
        truncated: bool,
        /// Where both streams were saved in full, relative to the
        /// workspace: add `.stdout` or `.stderr`.
        saved_output: String,
        stdout_preview: String,
        stderr_preview: String,
    },
    CompactionStarted,
    CompactionFinished {
        covered_messages: usize,
        used_llm: bool,
    },
}
