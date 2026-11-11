pub const ANTHROPIC_MESSAGES_API: &str = "https://api.anthropic.com/v1/messages";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
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
pub const SUMMARY_STATE_FILE_NAME: &str = "summaries.json";
pub const EDIT_HISTORY_FILE_NAME: &str = "edit_history.json";
pub const MAX_EDIT_HISTORY_ENTRIES: usize = 200;
pub const MAX_PERSISTED_MESSAGES: usize = 2_000;
pub const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 200_000;
pub const DEFAULT_NON_ANTHROPIC_CONTEXT_WINDOW_TOKENS: u32 = 128_000;
pub const TARGET_RECENT_CONTEXT_MESSAGES: usize = 24;
pub const MIN_RECENT_CONTEXT_MESSAGES: usize = 8;
pub const MAX_SUMMARY_LINES: usize = 24;
pub const MAX_SUMMARY_LINE_CHARACTERS: usize = 240;
pub const MAX_SUMMARY_CHARACTERS: usize = 4_000;
// Compaction: summarize old conversation with a cheap dedicated model so
// long sessions stop resending their full history on every call.
// Auto-compaction fires when the last measured prompt size exceeds
// COMPACTION_TRIGGER_RATIO of the context window (override with the
// COMPACTION_TOKEN_THRESHOLD env var). COMPACTION_RETAIN_RECENT_MESSAGES
// messages are kept verbatim; TARGET/MIN_RECENT_CONTEXT_MESSAGES remain
// the emergency budget-based trimming path for uncompacted overflow.
pub const COMPACTION_TRIGGER_RATIO: f32 = 0.8;
pub const COMPACTION_RETAIN_RECENT_MESSAGES: usize = 16;
pub const DEFAULT_COMPACTION_MODEL: &str = "claude-haiku-4-5";
pub const COMPACTION_MAX_OUTPUT_TOKENS: u32 = 1_500;
pub const COMPACTION_SOURCE_MESSAGE_MAX_CHARACTERS: usize = 600;
pub const COMPACTION_SYSTEM_PROMPT: &str = "You compact the conversation history of Socium, a coding agent. You receive the transcript of an agent session, possibly preceded by a summary of even earlier conversation in <previous_summary>. Merge everything into one summary with exactly these five sections:\n1. User request\n2. Files touched (use path:line where known)\n3. Decisions\n4. Errors and unresolved issues\n5. Immediate next step\nBe specific and concise. Preserve exact file paths, command names, identifiers, and error messages. Output only the five numbered sections, with no preamble and no commentary.";

pub const DEFAULT_SYSTEM_PROMPT: &str = "You are Socium, a coding agent working inside the user's current workspace. Be concise and concrete. When you refer to repository code, cite exact locations using path:line notation such as src/main.rs:42. If you discuss a specific function, edit, bug, or search result, include at least one line-addressable reference when the exact line is available from the repo or tool output.";
