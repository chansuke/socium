pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
// Thinking cannot be turned off on the default model and counts toward
// max_tokens, so leave room for it as well as the visible answer.
pub const DEFAULT_MAX_TOKENS: u32 = 64_000;
pub const MAX_READ_FILE_BYTES: usize = 200 * 1024;
pub const MAX_TOOL_ROUNDS: usize = 20;
pub const MAX_LIST_ENTRIES: usize = 1000;
pub const DEFAULT_SEARCH_RESULTS: usize = 50;
pub const MAX_SEARCH_RESULTS: usize = 200;
pub const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 30_000;
pub const MAX_COMMAND_OUTPUT_CHARACTERS: usize = 20_000;
pub const TEXT_EDITOR_TOOL_NAME: &str = "str_replace_based_edit_tool";
// Keep this aligned with Anthropic text editor tool output limits.
pub const MAX_TEXT_EDITOR_VIEW_CHARACTERS: usize = 50_000;
pub const MAX_TEXT_EDITOR_VIEW_READ_BYTES: usize = 4 * 1024 * 1024;
pub const STATE_DIR_NAME: &str = ".socium";
pub const CONVERSATION_STATE_FILE_NAME: &str = "conversation.json";
pub const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 200_000;
pub const TARGET_RECENT_CONTEXT_MESSAGES: usize = 24;
/// One tool result is cut to its head and tail beyond this, so a single
/// call cannot fill the window.
pub const MAX_TOOL_RESULT_CHARACTERS: usize = 100_000;
/// A result shrunk to fit the window keeps at least this much.
pub const MIN_SHRUNK_TOOL_RESULT_CHARACTERS: usize = 2_000;
pub const MAX_SUMMARY_LINES: usize = 24;
pub const MAX_SUMMARY_LINE_CHARACTERS: usize = 240;
pub const MAX_SUMMARY_CHARACTERS: usize = 4_000;
// Compaction: summarize old conversation with a cheap dedicated model so
// long sessions stop resending their full history on every call.
// Auto-compaction fires when the last measured prompt size reaches the
// model's compact_at_tokens, DEFAULT_COMPACT_AT_TOKENS, or
// COMPACTION_TRIGGER_RATIO of the input budget, whichever is lowest
// (override with the COMPACTION_TOKEN_THRESHOLD env var). Up to
// COMPACTION_RETAIN_RECENT_MESSAGES of the newest messages are kept
// verbatim, as long as they fit in COMPACTION_RETAIN_RECENT_TOKENS;
// TARGET_RECENT_CONTEXT_MESSAGES is the emergency budget-based trimming
// path for uncompacted overflow.
pub const COMPACTION_TRIGGER_RATIO: f32 = 0.8;
/// Long prompts cost more per round and the model attends to them less
/// well, so a 1M window is not filled before compacting.
pub const DEFAULT_COMPACT_AT_TOKENS: u32 = 200_000;
pub const COMPACTION_RETAIN_RECENT_MESSAGES: usize = 16;
pub const COMPACTION_RETAIN_RECENT_TOKENS: u32 = 20_000;
pub const DEFAULT_COMPACTION_MODEL: &str = "claude-haiku-5-5";
pub const COMPACTION_MAX_OUTPUT_TOKENS: u32 = 1_500;
pub const COMPACTION_SOURCE_MESSAGE_MAX_CHARACTERS: usize = 600;
pub const COMPACTION_SYSTEM_PROMPT: &str = "You compact the conversation history of Socium, a coding agent. You receive the transcript of an agent session, possibly preceded by a summary of even earlier conversation in <previous_summary>. Merge everything into one summary with exactly these five sections:\n1. User request\n2. Files touched (use path:line where known)\n3. Decisions\n4. Errors and unresolved issues\n5. Immediate next step\nBe specific and concise. Preserve exact file paths, command names, identifiers, and error messages. Output only the five numbered sections, with no preamble and no commentary.";

pub const DEFAULT_SYSTEM_PROMPT: &str = "You are Socium, a coding agent working inside the user's current workspace. Be concise and concrete. When you refer to repository code, cite exact locations using path:line notation such as src/main.rs:42. If you discuss a specific function, edit, bug, or search result, include at least one line-addressable reference when the exact line is available from the repo or tool output.";

// Subagents: the `agent` tool runs a task in a fresh context with its own
// tool loop and returns only the final report, cut to this many characters.
pub const AGENT_TOOL_NAME: &str = "agent";
pub const MAX_SUBAGENT_REPORT_CHARACTERS: usize = 20_000;
pub const SUBAGENT_EXPLORER_SYSTEM_PROMPT: &str = "You are an explorer subagent of Socium, a coding agent, working inside the user's current workspace. Another agent delegated one task to you; you cannot see its conversation, and it sees only your final message. Your tools are read-only: read, list, and search files and inspect Git history, but never try to change anything. Search broadly first, then read what matters; stop as soon as you can answer. Finish with a concise, self-contained report that answers the task, citing exact locations as path:line (for example src/main.rs:42), and say plainly what you could not find or verify.";
pub const SUBAGENT_WORKER_SYSTEM_PROMPT: &str = "You are a worker subagent of Socium, a coding agent, working inside the user's current workspace. Another agent delegated one task to you; you cannot see its conversation, and it sees only your final message. Do exactly the task you were given and nothing beyond it. Commands and commits still need the user's approval; if one is denied, do not retry it, and report it. Preserve the user's existing changes. Finish with a concise, self-contained report: what you changed, with path:line references, how you verified it, and anything left undone or uncertain.";
