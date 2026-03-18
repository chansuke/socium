pub mod agent;
pub mod constants;
pub mod providers;
pub mod tools;

use crate::agent::state::{
    ConversationState, ConversationStateRef, load_json_file, save_json_file,
};
use crate::agent::turn::{extract_text, run_agent_turn};
use crate::agent::types::Message;
use crate::constants::*;
use crate::providers::{ProviderConfig, ProviderKind};
use crate::tools::{ToolDefinition, default_tools};
use std::env;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use std::time::Duration;

pub struct AgentReply {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    pub role: TranscriptRole,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentProgress {
    ModelRequestStarted {
        round: usize,
        provider: String,
    },
    ApprovalRequired {
        tool: String,
        reason: String,
    },
    AssistantText {
        text: String,
    },
    ToolCallStarted {
        name: String,
    },
    ToolCallFinished {
        name: String,
        is_error: bool,
    },
    CommandStarted {
        command: String,
        cwd: String,
    },
    CommandFinished {
        command: String,
        exit_code: Option<i32>,
        timed_out: bool,
    },
}

pub struct AgentService {
    workspace_root: PathBuf,
    conversation_path: PathBuf,
    messages: Vec<Message>,
    provider: ProviderKind,
    provider_config: ProviderConfig,
    supports_tools: bool,
    model: String,
    max_tokens: u32,
    tools: Vec<ToolDefinition>,
    client: reqwest::Client,
}

impl AgentService {
    pub fn from_current_dir() -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_env(env::current_dir()?)
    }

    pub fn from_env(workspace_root: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let conversation_path = workspace_root
            .join(STATE_DIR_NAME)
            .join(CONVERSATION_STATE_FILE_NAME);

        let messages = match load_json_file::<ConversationState>(&conversation_path) {
            Ok(state) => {
                let mut messages = state.messages;
                if messages.len() > MAX_PERSISTED_MESSAGES {
                    let overflow = messages.len() - MAX_PERSISTED_MESSAGES;
                    messages.drain(0..overflow);
                    eprintln!(
                        "Persisted conversation exceeded {} messages and was truncated.",
                        MAX_PERSISTED_MESSAGES
                    );
                }
                messages
            }
            Err(err) => {
                eprintln!(
                    "Failed to load persisted conversation '{}': {err}. Starting with an empty conversation.",
                    conversation_path.display()
                );
                Vec::new()
            }
        };

        let provider = ProviderKind::from_env()?;
        let provider_config = provider.config_from_env()?;
        let model = env::var("MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string());
        let max_tokens = env::var("MAX_TOKENS")
            .unwrap_or_else(|_| "1000".to_string())
            .parse::<u32>()
            .map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("MAX_TOKENS must be an unsigned integer: {e}"),
                )
            })?;
        let tools = default_tools();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .build()?;

        Ok(Self {
            workspace_root,
            conversation_path,
            messages,
            provider,
            provider_config,
            supports_tools: provider.capabilities().supports_tools,
            model,
            max_tokens,
            tools,
            client,
        })
    }

    pub fn clear(&mut self) -> std::io::Result<()> {
        self.messages.clear();
        self.persist_messages()
    }

    pub fn transcript_entries(&self) -> Vec<TranscriptEntry> {
        self.messages
            .iter()
            .filter_map(|message| {
                let text = match &message.content {
                    crate::agent::types::MessageContent::Text(text) => text.clone(),
                    crate::agent::types::MessageContent::Blocks(blocks) => extract_text(blocks),
                };
                if text.trim().is_empty() {
                    return None;
                }
                let role = match message.role {
                    crate::agent::types::Role::User => TranscriptRole::User,
                    crate::agent::types::Role::Assistant => TranscriptRole::Assistant,
                    crate::agent::types::Role::System => TranscriptRole::System,
                };
                Some(TranscriptEntry { role, text })
            })
            .collect()
    }

    pub async fn submit(
        &mut self,
        user_input: String,
    ) -> Result<AgentReply, Box<dyn std::error::Error>> {
        self.submit_with_progress(user_input, |_| {}).await
    }

    pub async fn submit_with_progress<F>(
        &mut self,
        user_input: String,
        mut on_progress: F,
    ) -> Result<AgentReply, Box<dyn std::error::Error>>
    where
        F: FnMut(AgentProgress),
    {
        self.messages.push(Message::user_text(user_input));
        let assistant_chunks = run_agent_turn(
            self.provider,
            &self.client,
            &self.provider_config,
            &self.model,
            self.max_tokens,
            self.supports_tools,
            &self.tools,
            &self.workspace_root,
            &mut self.messages,
            &mut on_progress,
        )
        .await?;
        self.persist_messages()?;
        Ok(AgentReply {
            text: assistant_chunks.join("\n"),
        })
    }

    fn persist_messages(&self) -> std::io::Result<()> {
        save_json_file(
            &self.conversation_path,
            &ConversationStateRef {
                messages: &self.messages,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::state::{load_json_file, save_json_file};
    use crate::agent::turn::extract_text;
    use crate::agent::types::{ContentBlock, Message, MessageContent, Role};
    use crate::constants::{
        CONVERSATION_STATE_FILE_NAME, MAX_COMMAND_TIMEOUT_MS, MAX_LIST_ENTRIES,
        MAX_READ_FILE_BYTES, MAX_TEXT_EDITOR_VIEW_READ_BYTES, STATE_DIR_NAME,
        TEXT_EDITOR_TOOL_NAME,
    };
    use crate::providers::{ProviderConfig, ProviderKind};
    use crate::tools::default_tools;
    use crate::tools::runtime::{
        find_symbol_in_workspace, git_branch_from_workspace, git_commit_from_workspace,
        git_diff_from_workspace, git_log_from_workspace, git_status_from_workspace,
        list_files_from_workspace, read_file_from_workspace, resolve_workspace_path,
        resolve_workspace_path_for_create, run_tool_call, run_tool_call_with_progress,
        run_tool_calls, search_text_from_workspace,
    };
    use crate::{AgentProgress, AgentService, TranscriptEntry, TranscriptRole};
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Debug, Serialize, Deserialize, Default)]
    struct InputHistoryState {
        entries: Vec<String>,
    }

    #[test]
    fn load_json_file_returns_default_for_missing_file() {
        let temp = make_temp_dir("json-default");
        let missing = temp.path().join("missing.json");

        let state = load_json_file::<InputHistoryState>(&missing).unwrap();

        assert!(state.entries.is_empty());
    }

    #[test]
    fn load_json_file_reports_invalid_data() {
        let temp = make_temp_dir("json-invalid");
        let path = temp.path().join("invalid.json");
        fs::write(&path, "{ not-valid-json }").unwrap();

        let err = load_json_file::<InputHistoryState>(&path).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("Invalid JSON"));
        assert!(err.to_string().contains("invalid.json"));
    }

    #[test]
    fn save_json_file_creates_parent_directory_and_round_trips() {
        let temp = make_temp_dir("json-save");
        let path = temp.path().join("nested/state/history.json");
        let state = InputHistoryState {
            entries: vec!["one".to_string(), "two".to_string()],
        };

        save_json_file(&path, &state).unwrap();

        assert!(path.exists());
        let loaded = load_json_file::<InputHistoryState>(&path).unwrap();
        assert_eq!(loaded.entries, state.entries);
    }

    #[test]
    fn transcript_entries_include_text_messages_and_skip_tool_only_blocks() {
        let workspace = make_temp_dir("transcript");
        let conversation_path = workspace
            .path()
            .join(STATE_DIR_NAME)
            .join(CONVERSATION_STATE_FILE_NAME);
        let messages = vec![
            Message {
                role: Role::User,
                content: MessageContent::Text("hello".to_string()),
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![
                    ContentBlock::Text {
                        text: "answer".to_string(),
                    },
                    ContentBlock::ToolUse {
                        id: "tool_1".to_string(),
                        name: "read_file".to_string(),
                        input: json!({"path": "src/lib.rs"}),
                    },
                ]),
            },
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "tool_1".to_string(),
                    content: "Path: src/lib.rs".to_string(),
                    is_error: None,
                }]),
            },
        ];

        let service = AgentService {
            workspace_root: workspace.path().to_path_buf(),
            conversation_path,
            messages,
            provider: ProviderKind::Anthropic,
            provider_config: ProviderConfig::Anthropic {
                api_key: "test".to_string(),
                anthropic_version: "2023-06-01".to_string(),
            },
            supports_tools: true,
            model: "test-model".to_string(),
            max_tokens: 128,
            tools: default_tools(),
            client: reqwest::Client::builder().build().unwrap(),
        };

        assert_eq!(
            service.transcript_entries(),
            vec![
                TranscriptEntry {
                    role: TranscriptRole::User,
                    text: "hello".to_string(),
                },
                TranscriptEntry {
                    role: TranscriptRole::Assistant,
                    text: "answer".to_string(),
                },
            ]
        );
    }

    #[test]
    fn reads_file_from_workspace() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("note.txt");
        fs::write(&file_path, "hello").unwrap();

        let output =
            read_file_from_workspace(workspace.path(), &json!({"path": "note.txt"})).unwrap();

        assert!(output.contains("Path:"));
        assert!(output.contains("hello"));
    }

    #[test]
    fn rejects_path_outside_workspace() {
        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let file_path = outside.path().join("secret.txt");
        fs::write(&file_path, "secret").unwrap();

        let output = read_file_from_workspace(
            workspace.path(),
            &json!({"path": file_path.to_string_lossy().to_string()}),
        );

        assert!(output.is_err());
    }

    #[test]
    fn rejects_too_large_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("large.txt");
        let large = vec![b'a'; MAX_READ_FILE_BYTES + 1];
        fs::write(&file_path, large).unwrap();

        let output = read_file_from_workspace(workspace.path(), &json!({"path": "large.txt"}));

        assert!(output.is_err());
    }

    #[test]
    fn lists_directory_from_workspace_with_default_path() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("a.txt"), "a").unwrap();
        fs::create_dir(workspace.path().join("dir")).unwrap();

        let output = list_files_from_workspace(workspace.path(), &json!({})).unwrap();

        assert!(output.contains("Path: ."));
        assert!(output.contains("[file] a.txt"));
        assert!(output.contains("[dir] dir/"));
    }

    #[test]
    fn lists_specific_subdirectory_from_workspace() {
        let workspace = make_temp_dir("workspace");
        let subdir = workspace.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("inside.txt"), "inside").unwrap();

        let output =
            list_files_from_workspace(workspace.path(), &json!({"path": "subdir"})).unwrap();

        assert!(output.contains("Path: subdir"));
        assert!(output.contains("[file] inside.txt"));
    }

    #[test]
    fn lists_file_path_from_workspace() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("one.txt"), "1").unwrap();

        let output =
            list_files_from_workspace(workspace.path(), &json!({"path": "one.txt"})).unwrap();

        assert_eq!(output, "Path: one.txt\n[file]");
    }

    #[test]
    fn lists_empty_directory_from_workspace() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir(workspace.path().join("empty")).unwrap();

        let output =
            list_files_from_workspace(workspace.path(), &json!({"path": "empty"})).unwrap();

        assert_eq!(output, "Path: empty\n(empty)");
    }

    #[test]
    fn search_text_returns_line_addressable_matches() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(
            workspace.path().join("src/lib.rs"),
            "pub fn alpha() {}\nlet needle = 1;\n",
        )
        .unwrap();
        fs::write(
            workspace.path().join("src/other.rs"),
            "const NEEDLE: &str = \"needle\";\n",
        )
        .unwrap();

        let output = search_text_from_workspace(
            workspace.path(),
            &json!({
                "pattern": "needle",
                "path": "src"
            }),
        )
        .unwrap();

        assert!(output.contains("Pattern: needle"));
        assert!(output.contains("Path: src"));
        assert!(output.contains("src/lib.rs:2:let needle = 1;"));
        assert!(output.contains("src/other.rs:1:const NEEDLE: &str = \"needle\";"));
    }

    #[test]
    fn search_text_respects_glob_and_max_results() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(workspace.path().join("src/a.rs"), "needle\n").unwrap();
        fs::write(workspace.path().join("src/b.rs"), "needle\n").unwrap();
        fs::write(workspace.path().join("src/c.txt"), "needle\n").unwrap();

        let output = search_text_from_workspace(
            workspace.path(),
            &json!({
                "pattern": "needle",
                "glob": "*.rs",
                "max_results": 1
            }),
        )
        .unwrap();

        assert!(output.contains("Glob: *.rs"));
        assert!(output.contains("src/a.rs:1:needle") || output.contains("src/b.rs:1:needle"));
        assert!(!output.contains("src/c.txt"));
        assert!(output.contains("(truncated: showing first 1 matches; more matches omitted)"));
    }

    #[test]
    fn list_files_supports_recursive_glob_filtering() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir_all(workspace.path().join("src/nested")).unwrap();
        fs::write(workspace.path().join("src/lib.rs"), "pub fn lib() {}\n").unwrap();
        fs::write(
            workspace.path().join("src/nested/mod.rs"),
            "pub mod nested;\n",
        )
        .unwrap();
        fs::write(workspace.path().join("src/nested/readme.txt"), "note\n").unwrap();

        let output = list_files_from_workspace(
            workspace.path(),
            &json!({
                "path": "src",
                "recursive": true,
                "glob": "*.rs"
            }),
        )
        .unwrap();

        assert!(output.contains("Path: src"));
        assert!(output.contains("Glob: *.rs"));
        assert!(output.contains("Recursive: true"));
        assert!(output.contains("[file] lib.rs"));
        assert!(output.contains("[file] nested/mod.rs"));
        assert!(!output.contains("readme.txt"));
    }

    #[test]
    fn find_symbol_returns_rust_declaration_matches() {
        let workspace = make_temp_dir("workspace");
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(
            workspace.path().join("src/lib.rs"),
            "pub struct Widget;\nfn helper() {}\nimpl Widget { pub fn build() -> Self { Self } }\n",
        )
        .unwrap();
        fs::write(
            workspace.path().join("src/other.rs"),
            "pub enum WidgetState { Ready }\n",
        )
        .unwrap();

        let output = find_symbol_in_workspace(
            workspace.path(),
            &json!({
                "symbol": "Widget"
            }),
        )
        .unwrap();

        assert!(output.contains("Symbol: Widget"));
        assert!(output.contains("src/lib.rs:1:pub struct Widget;"));
        assert!(output.contains("src/lib.rs:3:impl Widget { pub fn build() -> Self { Self } }"));
        assert!(!output.contains("WidgetState"));
    }

    #[test]
    fn apply_patch_updates_multiple_hunks_in_one_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("notes.txt");
        fs::write(&file_path, "one\ntwo\nthree\nfour\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_update",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Update File: notes.txt\n@@\n one\n-two\n+TWO\n three\n@@\n three\n-four\n+FOUR\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Updated file: notes.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        let updated = fs::read_to_string(&file_path).unwrap();
        assert_eq!(updated, "one\nTWO\nthree\nFOUR\n");
    }

    #[test]
    fn apply_patch_creates_new_file() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_add",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Add File: created.txt\n+hello\n+world\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Added file: created.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        let created = fs::read_to_string(workspace.path().join("created.txt")).unwrap();
        assert_eq!(created, "hello\nworld");
    }

    #[test]
    fn apply_patch_deletes_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("remove.txt");
        fs::write(&file_path, "delete me").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_delete",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Delete File: remove.txt\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Deleted file: remove.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        assert!(!file_path.exists());
    }

    #[test]
    fn apply_patch_moves_file_and_updates_contents() {
        let workspace = make_temp_dir("workspace");
        let from_path = workspace.path().join("old.txt");
        fs::write(&from_path, "alpha\nbeta\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_move",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Update File: old.txt\n*** Move to: new.txt\n@@\n alpha\n-beta\n+gamma\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Moved file: old.txt -> new.txt"));
                assert!(content.contains("Updated file: new.txt"));
            }
            _ => panic!("expected tool_result"),
        }

        assert!(!from_path.exists());
        let moved = fs::read_to_string(workspace.path().join("new.txt")).unwrap();
        assert_eq!(moved, "alpha\ngamma\n");
    }

    #[test]
    fn apply_patch_dry_run_returns_preview_without_writing() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("preview.txt");
        fs::write(&file_path, "before\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_patch_preview",
            "apply_patch",
            &json!({
                "dry_run": true,
                "patch": "*** Begin Patch\n*** Update File: preview.txt\n@@\n-before\n+after\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Preview: true"));
                assert!(content.contains("--- preview.txt"));
                assert!(content.contains("+++ preview.txt"));
                assert!(content.contains("-before"));
                assert!(content.contains("+after"));
            }
            _ => panic!("expected tool_result"),
        }

        let unchanged = fs::read_to_string(&file_path).unwrap();
        assert_eq!(unchanged, "before\n");
    }

    #[test]
    fn git_branch_returns_current_branch() {
        let workspace = make_temp_dir("git-branch");
        init_git_repo(workspace.path());

        let output = git_branch_from_workspace(workspace.path(), &json!({})).unwrap();

        assert!(output.contains("Branch: main"));
    }

    #[test]
    fn git_status_reports_branch_and_dirty_entries() {
        let workspace = make_temp_dir("git-status");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();
        fs::write(workspace.path().join("untracked.txt"), "new\n").unwrap();

        let output = git_status_from_workspace(workspace.path(), &json!({})).unwrap();

        assert!(output.contains("Branch: main"));
        assert!(output.contains(" M tracked.txt"));
        assert!(output.contains("?? untracked.txt"));
    }

    #[test]
    fn git_diff_returns_patch_for_requested_path() {
        let workspace = make_temp_dir("git-diff");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();

        let output =
            git_diff_from_workspace(workspace.path(), &json!({ "path": "tracked.txt" })).unwrap();

        assert!(output.contains("Path: tracked.txt"));
        assert!(output.contains("--- a/tracked.txt"));
        assert!(output.contains("+++ b/tracked.txt"));
        assert!(output.contains("-initial"));
        assert!(output.contains("+changed"));
    }

    #[test]
    fn git_log_returns_recent_commits() {
        let workspace = make_temp_dir("git-log");
        init_git_repo(workspace.path());

        let output = git_log_from_workspace(workspace.path(), &json!({ "max_count": 1 })).unwrap();

        assert!(output.contains("Count: 1"));
        assert!(output.contains("initial commit"));
    }

    #[test]
    fn text_editor_create_warns_when_git_worktree_is_dirty() {
        let workspace = make_temp_dir("git-dirty-create");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "dirty\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_dirty_create",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "create",
                "path": "new.txt",
                "file_text": "created"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Created file"));
                assert!(content.contains("Warning: git worktree has uncommitted changes"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn apply_patch_warns_when_git_worktree_is_dirty() {
        let workspace = make_temp_dir("git-dirty-patch");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "dirty\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_dirty_patch",
            "apply_patch",
            &json!({
                "patch": "*** Begin Patch\n*** Add File: another.txt\n+hello\n*** End Patch\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Added file: another.txt"));
                assert!(content.contains("Warning: git worktree has uncommitted changes"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn git_commit_requires_explicit_confirmation() {
        let workspace = make_temp_dir("git-commit-confirm");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_git_commit_denied",
            "git_commit",
            &json!({
                "message": "save work"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("requires explicit confirmation"));
            }
            _ => panic!("expected tool_result"),
        }

        let head_message = git_stdout(workspace.path(), &["log", "-1", "--pretty=%s"]);
        assert_eq!(head_message.trim(), "initial commit");
    }

    #[test]
    fn git_commit_creates_commit_when_confirmed() {
        let workspace = make_temp_dir("git-commit-ok");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();

        let output = git_commit_from_workspace(
            workspace.path(),
            &json!({
                "message": "save work",
                "confirmed": true
            }),
        )
        .unwrap();

        assert!(output.contains("Commit:"));
        assert!(output.contains("Message: save work"));

        let head_message = git_stdout(workspace.path(), &["log", "-1", "--pretty=%s"]);
        assert_eq!(head_message.trim(), "save work");

        let status = git_stdout(workspace.path(), &["status", "--short"]);
        assert!(status.trim().is_empty());
    }

    #[test]
    fn run_tool_call_returns_success_for_list_files_without_path() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("one.txt"), "1").unwrap();

        let result = run_tool_call(workspace.path(), "tool_4", "list_files", &json!({}));

        match result {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_4");
                assert!(content.contains("Path: ."));
                assert!(content.contains("one.txt"));
                assert_eq!(is_error, None);
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn extract_text_ignores_non_text_blocks() {
        let blocks = vec![
            ContentBlock::Text {
                text: "first".to_string(),
            },
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "ignored.txt"}),
            },
            ContentBlock::Text {
                text: "second".to_string(),
            },
        ];

        let text = extract_text(&blocks);

        assert_eq!(text, "first\nsecond");
    }

    #[test]
    fn run_tool_call_returns_error_for_unsupported_tool() {
        let workspace = make_temp_dir("workspace");
        let result = run_tool_call(workspace.path(), "tool_1", "unknown_tool", &json!({}));

        match result {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_1");
                assert!(content.contains("Unsupported tool"));
                assert_eq!(is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_tool_call_returns_success_for_read_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("ok.txt");
        fs::write(&file_path, "ok").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_3",
            "read_file",
            &json!({"path": "ok.txt"}),
        );

        match result {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_3");
                assert!(content.contains("Path: ok.txt"));
                assert!(content.contains("ok"));
                assert_eq!(is_error, None);
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_tool_call_returns_success_for_text_editor_view() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("edit.txt"), "first\nsecond\nthird\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_1",
            TEXT_EDITOR_TOOL_NAME,
            &json!({"command": "view", "path": "edit.txt", "view_range": [2, -1]}),
        );

        match result {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_te_1");
                assert!(content.contains("Path: edit.txt"));
                assert!(content.contains("second\nthird"));
                assert_eq!(is_error, None);
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_view_rejects_invalid_negative_end_line() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("edit.txt"), "first\nsecond\nthird\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_view_neg_end",
            TEXT_EDITOR_TOOL_NAME,
            &json!({"command": "view", "path": "edit.txt", "view_range": [1, -2]}),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("end must be -1 or a positive integer"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_view_range_preserves_line_endings() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("crlf.txt");
        fs::write(&file_path, b"line1\r\nline2\r\nline3\r\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_view_crlf",
            TEXT_EDITOR_TOOL_NAME,
            &json!({"command": "view", "path": "crlf.txt", "view_range": [2, 2]}),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert_eq!(content, "Path: crlf.txt\nline2\r\n");
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_view_large_file_is_truncated_instead_of_rejected() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("large-view.txt");
        let large = vec![b'a'; MAX_READ_FILE_BYTES + 1];
        fs::write(&file_path, large).unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_view_large",
            TEXT_EDITOR_TOOL_NAME,
            &json!({"command": "view", "path": "large-view.txt"}),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Path: large-view.txt"));
                assert!(content.contains("(truncated: showing first"));
                assert!(!content.contains("File too large"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_view_rejects_extremely_large_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("too-large-view.txt");
        let huge = vec![b'a'; MAX_TEXT_EDITOR_VIEW_READ_BYTES + 1];
        fs::write(&file_path, huge).unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_view_too_large",
            TEXT_EDITOR_TOOL_NAME,
            &json!({"command": "view", "path": "too-large-view.txt"}),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("File too large"));
                assert!(content.contains("too-large-view.txt"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_str_replace_updates_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("replace.txt");
        fs::write(&file_path, "hello old world").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_2",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "str_replace",
                "path": "replace.txt",
                "old_str": "old",
                "new_str": "new"
            }),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }
        let updated = fs::read_to_string(&file_path).unwrap();
        assert_eq!(updated, "hello new world");
    }

    #[test]
    fn text_editor_str_replace_rejects_multiple_matches() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("replace.txt"), "dup\ndup\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_3",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "str_replace",
                "path": "replace.txt",
                "old_str": "dup",
                "new_str": "x"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("Multiple matches found"));
                assert_eq!(is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_create_creates_new_file() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_te_4",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "create",
                "path": "new.txt",
                "file_text": "created"
            }),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }
        let content = fs::read_to_string(workspace.path().join("new.txt")).unwrap();
        assert_eq!(content, "created");
    }

    #[test]
    fn text_editor_create_creates_missing_parent_directories() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_te_create_nested",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "create",
                "path": "newdir/inner/new.txt",
                "file_text": "created"
            }),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }
        let content = fs::read_to_string(workspace.path().join("newdir/inner/new.txt")).unwrap();
        assert_eq!(content, "created");
    }

    #[test]
    fn text_editor_create_rejects_existing_path() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("exists.txt"), "keep").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_5",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "create",
                "path": "exists.txt",
                "file_text": "overwrite"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("already exists"));
                assert_eq!(is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_insert_inserts_after_line() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("insert.txt");
        fs::write(&file_path, "a\nb").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_6",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "insert",
                "path": "insert.txt",
                "insert_line": 1,
                "insert_text": "X\n"
            }),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }
        let updated = fs::read_to_string(&file_path).unwrap();
        assert_eq!(updated, "a\nX\nb");
    }

    #[test]
    fn text_editor_insert_rejects_out_of_range_line() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("insert.txt"), "a\nb").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_7",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "insert",
                "path": "insert.txt",
                "insert_line": 3,
                "insert_text": "X\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("out of range"));
                assert_eq!(is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_insert_requires_insert_text_field() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("insert.txt"), "a\nb").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_insert_missing_text",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "insert",
                "path": "insert.txt",
                "insert_line": 1,
                "new_str": "X\n"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("requires a string field: insert_text"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn text_editor_undo_edit_restores_previous_file_content() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("undo.txt");
        fs::write(&file_path, "before").unwrap();

        let edit_result = run_tool_call(
            workspace.path(),
            "tool_te_undo_edit_before",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "str_replace",
                "path": "undo.txt",
                "old_str": "before",
                "new_str": "after"
            }),
        );

        match edit_result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }

        let result = run_tool_call(
            workspace.path(),
            "tool_te_8",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "undo_edit",
                "path": "undo.txt"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("Undid last edit"));
                assert_eq!(is_error, None);
            }
            _ => panic!("expected tool_result"),
        }

        let restored = fs::read_to_string(&file_path).unwrap();
        assert_eq!(restored, "before");
    }

    #[test]
    fn text_editor_undo_edit_removes_last_created_file() {
        let workspace = make_temp_dir("workspace");

        let create_result = run_tool_call(
            workspace.path(),
            "tool_te_undo_create_before",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "create",
                "path": "created.txt",
                "file_text": "new"
            }),
        );

        match create_result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }

        let undo_result = run_tool_call(
            workspace.path(),
            "tool_te_undo_create",
            TEXT_EDITOR_TOOL_NAME,
            &json!({
                "command": "undo_edit",
                "path": "created.txt"
            }),
        );

        match undo_result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(content.contains("Undid last edit"));
                assert_eq!(is_error, None);
            }
            _ => panic!("expected tool_result"),
        }

        assert!(!workspace.path().join("created.txt").exists());
    }

    #[test]
    fn run_tool_calls_returns_error_for_invalid_read_file_input() {
        let workspace = make_temp_dir("workspace");
        let blocks = vec![ContentBlock::ToolUse {
            id: "tool_2".to_string(),
            name: "read_file".to_string(),
            input: json!({}),
        }];

        let results = run_tool_calls(workspace.path(), &blocks);

        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_2");
                assert!(content.contains("read_file requires a string field: path"));
                assert_eq!(*is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_tool_calls_returns_error_for_invalid_list_files_input() {
        let workspace = make_temp_dir("workspace");
        let blocks = vec![ContentBlock::ToolUse {
            id: "tool_5".to_string(),
            name: "list_files".to_string(),
            input: json!({"path": 1}),
        }];

        let results = run_tool_calls(workspace.path(), &blocks);

        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_5");
                assert!(content.contains("list_files requires an optional string field: path"));
                assert_eq!(*is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_tool_calls_returns_error_for_non_object_list_files_input() {
        let workspace = make_temp_dir("workspace");
        let blocks = vec![ContentBlock::ToolUse {
            id: "tool_6".to_string(),
            name: "list_files".to_string(),
            input: json!(["not", "an", "object"]),
        }];

        let results = run_tool_calls(workspace.path(), &blocks);

        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_6");
                assert!(content.contains("list_files requires an object input"));
                assert_eq!(*is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn resolve_rejects_relative_escape_path() {
        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();
        let escape = format!(
            "../{}/secret.txt",
            outside.path().file_name().unwrap().to_string_lossy()
        );

        let result = resolve_workspace_path(workspace.path(), &escape);

        assert!(result.is_err());
        assert!(result.err().unwrap().contains("outside the workspace"));
    }

    #[test]
    fn resolve_accepts_dot_components_inside_workspace() {
        let workspace = make_temp_dir("workspace");
        let dir = workspace.path().join("dir");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file.txt");
        fs::write(&file, "inside").unwrap();

        let resolved = resolve_workspace_path(workspace.path(), "dir/./../dir/file.txt").unwrap();

        assert_eq!(resolved, file.canonicalize().unwrap());
    }

    #[test]
    fn resolve_fails_for_nonexistent_path() {
        let workspace = make_temp_dir("workspace");

        let result = resolve_workspace_path(workspace.path(), "missing.txt");

        assert!(result.is_err());
        assert!(result.err().unwrap().starts_with("Failed to resolve path"));
    }

    #[test]
    fn resolve_rejects_absolute_path() {
        let workspace = make_temp_dir("workspace");
        let inside_file = workspace.path().join("inside.txt");
        fs::write(&inside_file, "inside").unwrap();

        let result = resolve_workspace_path(workspace.path(), &inside_file.to_string_lossy());

        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .contains("Absolute paths are not allowed")
        );
    }

    #[test]
    fn resolve_for_create_accepts_missing_file_in_workspace() {
        let workspace = make_temp_dir("workspace");

        let resolved =
            resolve_workspace_path_for_create(workspace.path(), "newdir/nested/missing.txt")
                .unwrap();

        assert_eq!(resolved, workspace.path().join("newdir/nested/missing.txt"));
    }

    #[test]
    fn resolve_for_create_rejects_escape_path() {
        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let escape = format!(
            "../{}/x.txt",
            outside.path().file_name().unwrap().to_string_lossy()
        );

        let result = resolve_workspace_path_for_create(workspace.path(), &escape);

        assert!(result.is_err());
        assert!(result.err().unwrap().contains("outside the workspace"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_for_create_rejects_dangling_symlink_path() {
        use std::os::unix::fs::symlink;

        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let missing_target = outside.path().join("missing.txt");
        let link = workspace.path().join("link.txt");
        symlink(&missing_target, &link).unwrap();

        let result = resolve_workspace_path_for_create(workspace.path(), "link.txt");

        assert!(result.is_err());
        assert!(result.err().unwrap().contains("dangling symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn list_files_marks_symlink_as_link() {
        use std::os::unix::fs::symlink;

        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("target.txt"), "target").unwrap();
        symlink(
            workspace.path().join("target.txt"),
            workspace.path().join("link.txt"),
        )
        .unwrap();

        let output = list_files_from_workspace(workspace.path(), &json!({})).unwrap();

        assert!(output.contains("[lnk] link.txt"));
        assert!(!output.contains("[file] link.txt"));
    }

    #[test]
    fn list_files_truncates_when_entries_exceed_limit() {
        let workspace = make_temp_dir("workspace");
        for i in 0..=MAX_LIST_ENTRIES {
            fs::write(workspace.path().join(format!("f{i:04}.txt")), "x").unwrap();
        }

        let output = list_files_from_workspace(workspace.path(), &json!({})).unwrap();
        let marker =
            format!("(truncated: showing first {MAX_LIST_ENTRIES} entries; more entries omitted)");

        assert!(output.contains(&marker));
    }

    #[cfg(unix)]
    #[test]
    fn run_command_runs_safe_command_in_requested_cwd() {
        let workspace = make_temp_dir("workspace");
        let nested = workspace.path().join("nested");
        fs::create_dir(&nested).unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_cmd_pwd",
            "run_command",
            &json!({
                "command": ["pwd"],
                "cwd": "nested"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Command: pwd"));
                assert!(content.contains("Cwd: nested"));
                assert!(content.contains("Permission: safe-read-only"));
                assert!(content.contains("Exit code: 0"));
                assert!(content.contains(nested.to_string_lossy().as_ref()));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_command_emits_tool_and_command_progress_events() {
        let workspace = make_temp_dir("workspace");
        let mut progress = Vec::new();

        let result = run_tool_call_with_progress(
            workspace.path(),
            "tool_cmd_progress",
            "run_command",
            &json!({
                "command": ["pwd"]
            }),
            &mut |event| progress.push(event),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }
        assert_eq!(
            progress,
            vec![
                AgentProgress::ToolCallStarted {
                    name: "run_command".to_string(),
                },
                AgentProgress::CommandStarted {
                    command: "pwd".to_string(),
                    cwd: ".".to_string(),
                },
                AgentProgress::CommandFinished {
                    command: "pwd".to_string(),
                    exit_code: Some(0),
                    timed_out: false,
                },
                AgentProgress::ToolCallFinished {
                    name: "run_command".to_string(),
                    is_error: false,
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_command_emits_approval_required_event_when_permission_is_missing() {
        let workspace = make_temp_dir("workspace");
        let mut progress = Vec::new();

        let result = run_tool_call_with_progress(
            workspace.path(),
            "tool_cmd_needs_approval",
            "run_command",
            &json!({
                "command": ["touch", "created.txt"]
            }),
            &mut |event| progress.push(event),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, Some(true)),
            _ => panic!("expected tool_result"),
        }
        assert_eq!(
            progress,
            vec![
                AgentProgress::ToolCallStarted {
                    name: "run_command".to_string(),
                },
                AgentProgress::ApprovalRequired {
                    tool: "run_command".to_string(),
                    reason: "workspace-write approval required".to_string(),
                },
                AgentProgress::ToolCallFinished {
                    name: "run_command".to_string(),
                    is_error: true,
                },
            ]
        );
    }

    #[test]
    fn git_commit_emits_approval_required_event_when_confirmation_is_missing() {
        let workspace = make_temp_dir("git-commit-needs-confirmation");
        init_git_repo(workspace.path());
        fs::write(workspace.path().join("tracked.txt"), "changed\n").unwrap();

        let mut progress = Vec::new();
        let result = run_tool_call_with_progress(
            workspace.path(),
            "tool_git_commit_needs_confirmation",
            "git_commit",
            &json!({
                "message": "test commit"
            }),
            &mut |event| progress.push(event),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, Some(true)),
            _ => panic!("expected tool_result"),
        }
        assert_eq!(
            progress,
            vec![
                AgentProgress::ToolCallStarted {
                    name: "git_commit".to_string(),
                },
                AgentProgress::ApprovalRequired {
                    tool: "git_commit".to_string(),
                    reason: "explicit confirmation required".to_string(),
                },
                AgentProgress::ToolCallFinished {
                    name: "git_commit".to_string(),
                    is_error: true,
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_command_requires_workspace_write_approval_and_runs_after_approval() {
        let workspace = make_temp_dir("workspace");

        let rejected = run_tool_call(
            workspace.path(),
            "tool_cmd_touch_denied",
            "run_command",
            &json!({
                "command": ["touch", "created.txt"]
            }),
        );

        match rejected {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("requires workspace-write approval"));
            }
            _ => panic!("expected tool_result"),
        }
        assert!(!workspace.path().join("created.txt").exists());

        let approved = run_tool_call(
            workspace.path(),
            "tool_cmd_touch_ok",
            "run_command",
            &json!({
                "command": ["touch", "created.txt"],
                "approval": "workspace-write"
            }),
        );

        match approved {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Permission: workspace-write"));
                assert!(content.contains("Exit code: 0"));
            }
            _ => panic!("expected tool_result"),
        }
        assert!(workspace.path().join("created.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn run_command_rejects_destructive_command_without_destructive_approval() {
        let workspace = make_temp_dir("workspace");
        let target = workspace.path().join("victim.txt");
        fs::write(&target, "keep").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_cmd_rm",
            "run_command",
            &json!({
                "command": ["rm", "victim.txt"],
                "approval": "workspace-write"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("requires destructive approval"));
            }
            _ => panic!("expected tool_result"),
        }
        assert!(target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn run_command_captures_stderr_and_non_zero_exit_code() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_cmd_sh_exit",
            "run_command",
            &json!({
                "command": ["sh", "-c", "printf 'out'; printf 'err' >&2; exit 7"],
                "approval": "workspace-write"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("Exit code: 7"));
                assert!(content.contains("Stdout:\nout"));
                assert!(content.contains("Stderr:\nerr"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_command_times_out() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_cmd_timeout",
            "run_command",
            &json!({
                "command": ["sh", "-c", "sleep 1"],
                "approval": "workspace-write",
                "timeout_ms": 10
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("Timed out: true"));
                assert!(content.contains("Timeout: 10 ms"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_command_rejects_timeout_above_limit() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_cmd_timeout_limit",
            "run_command",
            &json!({
                "command": ["pwd"],
                "timeout_ms": MAX_COMMAND_TIMEOUT_MS + 1
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("timeout_ms must be between"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_outside_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();
        let link = workspace.path().join("link.txt");
        symlink(&outside_file, &link).unwrap();

        let output = read_file_from_workspace(workspace.path(), &json!({"path": "link.txt"}));

        assert!(output.is_err());
        assert!(output.err().unwrap().contains("outside the workspace"));
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn make_temp_dir(prefix: &str) -> TempDir {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let count = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("socium_{prefix}_{nanos}_{count}"));
        fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }

    fn init_git_repo(path: &Path) {
        run_git(path, &["init", "-b", "main"]);
        run_git(path, &["config", "user.email", "socium@example.com"]);
        run_git(path, &["config", "user.name", "Socium Tests"]);
        fs::write(path.join("tracked.txt"), "initial\n").unwrap();
        run_git(path, &["add", "tracked.txt"]);
        run_git(path, &["commit", "-m", "initial commit"]);
    }

    fn run_git(path: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(path: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}
