pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
// Thinking cannot be turned off on the default model and counts toward
// max_tokens, so leave room for it as well as the visible answer.
pub const DEFAULT_MAX_TOKENS: u32 = 64_000;
pub const MAX_READ_FILE_BYTES: usize = 200 * 1024;
/// Lines `read_file` returns when no limit is given.
pub const READ_FILE_DEFAULT_LINES: usize = 2_000;
/// Characters one `read_file` or `view` result holds; the rest is read
/// with the next offset.
pub const MAX_READ_OUTPUT_CHARACTERS: usize = 80_000;
/// A longer line (minified code, data) is cut in read and search results.
pub const MAX_LINE_CHARACTERS: usize = 2_000;
/// Diff lines an edit result shows; the change itself is never cut.
pub const MAX_EDIT_DIFF_LINES: usize = 80;
pub const MAX_TOOL_ROUNDS: usize = 20;
pub const MAX_LIST_ENTRIES: usize = 1000;
pub const DEFAULT_SEARCH_RESULTS: usize = 50;
pub const MAX_SEARCH_RESULTS: usize = 200;
pub const MAX_SEARCH_LINE_CHARACTERS: usize = 500;
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
// Room for the summary and the compaction model's own thinking, which
// counts toward the limit.
pub const COMPACTION_MAX_OUTPUT_TOKENS: u32 = 8_000;
pub const COMPACTION_SOURCE_MESSAGE_MAX_CHARACTERS: usize = 600;
/// User messages carry requirements, so they go to the compaction model
/// nearly whole.
pub const COMPACTION_USER_MESSAGE_MAX_CHARACTERS: usize = 8_000;
/// The whole transcript sent for compaction; the middle goes first.
pub const COMPACTION_TRANSCRIPT_MAX_CHARACTERS: usize = 400_000;
pub const MAX_FIRST_REQUEST_CHARACTERS: usize = 8_000;
pub const COMPACTION_SYSTEM_PROMPT: &str = "You compact the conversation history of Socium, a coding agent. You receive the transcript of an agent session, possibly preceded by a summary of even earlier conversation in <previous_summary>. Merge everything into one summary with exactly these five sections:\n1. User request\n2. Files touched (use path:line where known)\n3. Decisions\n4. Errors and unresolved issues\n5. Immediate next step\nBe specific and concise. Preserve exact file paths, command names, identifiers, and error messages. Output only the five numbered sections, with no preamble and no commentary.";

pub const DEFAULT_SYSTEM_PROMPT: &str = "You are Socium, a coding agent working inside the user's current workspace. Be concise and concrete.

Code: read files before you change them. Read and view results number every line, so cite code as path:line (for example src/main.rs:42) whenever you discuss a function, edit, bug, or search result. Edit with apply_patch or the text editor; each edit result shows its diff. Keep the user's uncommitted changes, and never undo work you did not make.

Verifying: after a change, run the project's build or tests with run_command when you can, and say what you ran, what passed, and what you could not verify.

Sandbox: commands run in a Linux sandbox without network access and without the user's home directory. The workspace is writable; Git metadata is read-only, so commit with git_commit. System runtimes are read-only. When a command fails because a tool, a dependency cache, or the network is missing, say so instead of working around it: the user can expose directories, PATH entries, and a longer time limit in ~/.config/socium/config.toml.

Approval: commands may wait for the user's approval, and commits, edits to files the host runs (build scripts, CI, editor and environment settings), and reads of secret files always do. If the user denies a call, do not retry it; ask, or take another approach.";

// Subagents: the `agent` tool runs a task in a fresh context with its own
// tool loop and returns only the final report, cut to this many characters.
pub const AGENT_TOOL_NAME: &str = "agent";
pub const MAX_SUBAGENT_REPORT_CHARACTERS: usize = 20_000;
pub const SUBAGENT_EXPLORER_SYSTEM_PROMPT: &str = "You are an explorer subagent of Socium, a coding agent, working inside the user's current workspace. Another agent delegated one task to you; you cannot see its conversation, and it sees only your final message. Your tools are read-only: read, list, and search files and inspect Git history, but never try to change anything. Search broadly first, then read what matters; stop as soon as you can answer. Finish with a concise, self-contained report that answers the task, citing exact locations as path:line (for example src/main.rs:42), and say plainly what you could not find or verify.";
pub const SUBAGENT_WORKER_SYSTEM_PROMPT: &str = "You are a worker subagent of Socium, a coding agent, working inside the user's current workspace. Another agent delegated one task to you; you cannot see its conversation, and it sees only your final message. Do exactly the task you were given and nothing beyond it. Commands and commits still need the user's approval; if one is denied, do not retry it, and report it. Preserve the user's existing changes. Finish with a concise, self-contained report: what you changed, with path:line references, how you verified it, and anything left undone or uncertain.";
