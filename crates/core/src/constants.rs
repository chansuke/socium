pub const ANTHROPIC_MESSAGES_API: &str = "https://api.anthropic.com/v1/messages";
pub const MAX_READ_FILE_BYTES: usize = 200 * 1024;
pub const MAX_TOOL_ROUNDS: usize = 20;
pub const MAX_LIST_ENTRIES: usize = 1000;
pub const DEFAULT_SEARCH_RESULTS: usize = 50;
pub const MAX_SEARCH_RESULTS: usize = 200;
pub const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 30_000;
pub const MAX_COMMAND_TIMEOUT_MS: u64 = 120_000;
pub const MAX_COMMAND_OUTPUT_CHARACTERS: usize = 20_000;
pub const TEXT_EDITOR_TOOL_NAME: &str = "str_replace_based_edit_tool";
// Keep this aligned with Anthropic text editor tool output limits.
pub const MAX_TEXT_EDITOR_VIEW_CHARACTERS: usize = 50_000;
pub const MAX_TEXT_EDITOR_VIEW_READ_BYTES: usize = 4 * 1024 * 1024;
pub const STATE_DIR_NAME: &str = ".socium";
pub const CONVERSATION_STATE_FILE_NAME: &str = "conversation.json";
pub const EDIT_HISTORY_FILE_NAME: &str = "edit_history.json";
pub const MAX_EDIT_HISTORY_ENTRIES: usize = 200;
pub const MAX_PERSISTED_MESSAGES: usize = 2_000;
