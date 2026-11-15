use crate::constants::MAX_TOOL_ROUNDS;
use crate::providers::error::ProviderError;

/// Error type for the public `AgentService` API.
///
/// Tool failures deliberately do not appear here: a failing tool becomes an
/// `is_error` tool result inside the turn, not a turn failure.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Task cancelled; completed tool results have been saved")]
    Cancelled,
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    State(#[from] std::io::Error),
    #[error("Tool execution reached the maximum rounds ({MAX_TOOL_ROUNDS}) and may be incomplete")]
    ToolRoundsExhausted,
    #[error("Nothing to compact: the conversation is already within the retained window")]
    NothingToCompact,
    /// The window filled during a tool loop; the message says what next.
    #[error("{0}")]
    ContextFull(String),
}
