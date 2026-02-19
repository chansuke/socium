pub const ANTHROPIC_MESSAGES_API: &str = "https://api.anthropic.com/v1/messages";
pub const MAX_READ_FILE_BYTES: usize = 200 * 1024;
pub const MAX_TOOL_ROUNDS: usize = 20;
pub const MAX_LIST_ENTRIES: usize = 1000;
pub const TEXT_EDITOR_TOOL_NAME: &str = "str_replace_based_edit_tool";
// Keep this aligned with Anthropic text editor tool output limits.
pub const MAX_TEXT_EDITOR_VIEW_CHARACTERS: usize = 50_000;
pub const MAX_TEXT_EDITOR_VIEW_READ_BYTES: usize = 4 * 1024 * 1024;
pub const STATE_DIR_NAME: &str = ".socium";
pub const CONVERSATION_STATE_FILE_NAME: &str = "conversation.json";
pub const INPUT_HISTORY_FILE_NAME: &str = "input_history.json";
pub const MAX_INPUT_HISTORY_ENTRIES: usize = 1_000;
pub const MAX_PERSISTED_MESSAGES: usize = 2_000;
#[cfg(unix)]
pub const ESCAPE_SEQUENCE_FIRST_BYTE_TIMEOUT_MS: i32 = 120;
#[cfg(unix)]
pub const ESCAPE_SEQUENCE_FOLLOWUP_TIMEOUT_MS: i32 = 120;
