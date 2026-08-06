use serde::Serialize;
use serde_json::{Value, json};

use crate::constants::TEXT_EDITOR_TOOL_NAME;
use crate::constants::{
    MAX_COMMAND_TIMEOUT_MS, MAX_SEARCH_RESULTS, MAX_TEXT_EDITOR_VIEW_CHARACTERS,
};

pub(crate) mod apply_patch;
pub(crate) mod edit_history;
pub(crate) mod find_symbol;
pub(crate) mod git;
pub(crate) mod input;
pub(crate) mod list_files;
pub(crate) mod read_file;
pub(crate) mod ripgrep;
pub(crate) mod run_command;
pub(crate) mod search_text;
pub(crate) mod text_editor;
pub(crate) mod text_file;
pub(crate) mod workspace;

use crate::AgentProgress;
use crate::agent::types::ContentBlock;
use crate::tools::apply_patch::apply_patch_from_workspace;
use crate::tools::find_symbol::find_symbol_in_workspace;
use crate::tools::git::{
    git_branch_from_workspace, git_commit_from_workspace_with_progress, git_diff_from_workspace,
    git_log_from_workspace, git_status_from_workspace,
};
use crate::tools::list_files::list_files_from_workspace;
use crate::tools::read_file::read_file_from_workspace;
use crate::tools::run_command::run_command_from_workspace_with_progress;
use crate::tools::search_text::search_text_from_workspace;
use crate::tools::text_editor::text_editor_from_workspace;
use std::path::Path;

#[derive(Debug, Serialize, Clone)]
#[serde(untagged)]
pub enum ToolDefinition {
    Custom(CustomToolDefinition),
    AnthropicTextEditor(AnthropicTextEditorToolDefinition),
}

#[derive(Debug, Serialize, Clone)]
pub struct CustomToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Serialize, Clone)]
pub struct AnthropicTextEditorToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_characters: Option<usize>,
}

pub fn read_file_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "read_file".to_string(),
        description: "Read a UTF-8 text file from the current workspace by relative path."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to a file in the workspace. Absolute paths are not allowed."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
    })
}

pub fn list_files_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "list_files".to_string(),
        description: "List files and directories at a given path. Supports optional recursive traversal and file glob filtering for repo navigation."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional relative path to a file or directory in the workspace. Defaults to the current directory."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "If true, recursively lists matching files beneath the path."
                },
                "glob": {
                    "type": "string",
                    "description": "Optional glob filter such as '*.rs' or 'src/**/*.ts'. Applies to file paths relative to the requested path."
                }
            },
            "additionalProperties": false
        }),
    })
}

pub fn search_text_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "search_text".to_string(),
        description: "Search repository text using ripgrep and return line-addressable matches in the form path:line:text."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Text or regex pattern to search for."
                },
                "path": {
                    "type": "string",
                    "description": "Optional relative file or directory path to limit the search scope."
                },
                "glob": {
                    "type": "string",
                    "description": "Optional ripgrep glob filter such as '*.rs'."
                },
                "max_results": {
                    "type": "integer",
                    "description": format!("Optional max number of matches to return. Must be between 1 and {MAX_SEARCH_RESULTS}.")
                }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
    })
}

pub fn apply_patch_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "apply_patch".to_string(),
        description: "Apply a structured patch with add, delete, or multi-hunk update operations inside the workspace."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "dry_run": {
                    "type": "boolean",
                    "description": "If true, validates the patch and returns a preview without writing files."
                },
                "patch": {
                    "type": "string",
                    "description": "Patch text using *** Begin Patch / *** End Patch blocks with Add File, Delete File, or Update File sections."
                }
            },
            "required": ["patch"],
            "additionalProperties": false
        }),
    })
}

pub fn git_branch_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "git_branch".to_string(),
        description: "Show the current Git branch for the workspace repository.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    })
}

pub fn git_status_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "git_status".to_string(),
        description:
            "Show Git status for the workspace repository, including branch and dirty files."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional relative path to limit status output to part of the workspace."
                }
            },
            "additionalProperties": false
        }),
    })
}

pub fn git_diff_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "git_diff".to_string(),
        description:
            "Show the current Git diff for the workspace repository or an optional relative path."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional relative path to limit the diff."
                }
            },
            "additionalProperties": false
        }),
    })
}

pub fn git_log_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "git_log".to_string(),
        description: "Show recent Git commit history for the workspace repository.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "max_count": {
                    "type": "integer",
                    "description": "Optional number of commits to return."
                }
            },
            "additionalProperties": false
        }),
    })
}

pub fn git_commit_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "git_commit".to_string(),
        description: "Create a Git commit for the current workspace, but only when the caller explicitly confirms the intent."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Commit message to use."
                },
                "confirmed": {
                    "type": "boolean",
                    "description": "Must be true to confirm explicit user intent for the commit."
                }
            },
            "required": ["message", "confirmed"],
            "additionalProperties": false
        }),
    })
}

pub fn find_symbol_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "find_symbol".to_string(),
        description: "Find declaration-like Rust symbol references for quick navigation. Returns line-addressable matches in the form path:line:text."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "symbol": {
                    "type": "string",
                    "description": "Rust symbol name to search for."
                },
                "path": {
                    "type": "string",
                    "description": "Optional relative file or directory path to limit the search scope."
                },
                "max_results": {
                    "type": "integer",
                    "description": format!("Optional max number of matches to return. Must be between 1 and {MAX_SEARCH_RESULTS}.")
                }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    })
}

pub fn run_command_tool() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: "run_command".to_string(),
        description: "Run a workspace command for search, build, test, lint, or local execution. Commands are passed as an array of executable plus arguments, not as a shell string. The runtime classifies commands as safe-read-only, workspace-write, or destructive and may require the approval field to match the required permission level."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "array",
                    "description": "Executable followed by arguments, each as a separate string.",
                    "items": {
                        "type": "string"
                    },
                    "minItems": 1
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional relative working directory inside the workspace. Defaults to the workspace root."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": format!("Optional timeout in milliseconds. Must be between 1 and {MAX_COMMAND_TIMEOUT_MS}.")
                },
                "approval": {
                    "type": "string",
                    "description": "Optional approval granted by the user when the runtime requires it.",
                    "enum": ["workspace-write", "destructive"]
                }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
    })
}

pub fn text_editor_tool() -> ToolDefinition {
    ToolDefinition::AnthropicTextEditor(AnthropicTextEditorToolDefinition {
        tool_type: "text_editor_20250728".to_string(),
        name: TEXT_EDITOR_TOOL_NAME.to_string(),
        max_characters: Some(MAX_TEXT_EDITOR_VIEW_CHARACTERS),
    })
}

pub fn default_tools() -> Vec<ToolDefinition> {
    vec![
        read_file_tool(),
        list_files_tool(),
        search_text_tool(),
        apply_patch_tool(),
        git_branch_tool(),
        git_status_tool(),
        git_diff_tool(),
        git_log_tool(),
        git_commit_tool(),
        find_symbol_tool(),
        run_command_tool(),
        text_editor_tool(),
    ]
}

#[allow(dead_code)]
pub(crate) fn run_tool_calls(workspace_root: &Path, blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    let mut noop = |_| {};
    run_tool_calls_with_progress(workspace_root, blocks, &mut noop)
}

pub(crate) fn run_tool_calls_with_progress<F>(
    workspace_root: &Path,
    blocks: &[ContentBlock],
    on_progress: &mut F,
) -> Vec<ContentBlock>
where
    F: FnMut(AgentProgress),
{
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(run_tool_call_with_progress(
                workspace_root,
                id,
                name,
                input,
                on_progress,
            )),
            _ => None,
        })
        .collect()
}

#[allow(dead_code)]
pub(crate) fn run_tool_call(
    workspace_root: &Path,
    tool_use_id: &str,
    name: &str,
    input: &Value,
) -> ContentBlock {
    let mut noop = |_| {};
    run_tool_call_with_progress(workspace_root, tool_use_id, name, input, &mut noop)
}

pub(crate) fn run_tool_call_with_progress<F>(
    workspace_root: &Path,
    tool_use_id: &str,
    name: &str,
    input: &Value,
    on_progress: &mut F,
) -> ContentBlock
where
    F: FnMut(AgentProgress),
{
    on_progress(AgentProgress::ToolCallStarted {
        name: name.to_string(),
    });
    let result = match name {
        "read_file" => read_file_from_workspace(workspace_root, input),
        "list_files" => list_files_from_workspace(workspace_root, input),
        "search_text" => search_text_from_workspace(workspace_root, input),
        "apply_patch" => apply_patch_from_workspace(workspace_root, input),
        "git_branch" => git_branch_from_workspace(workspace_root, input),
        "git_status" => git_status_from_workspace(workspace_root, input),
        "git_diff" => git_diff_from_workspace(workspace_root, input),
        "git_log" => git_log_from_workspace(workspace_root, input),
        "git_commit" => git_commit_from_workspace_with_progress(workspace_root, input, on_progress),
        "find_symbol" => find_symbol_in_workspace(workspace_root, input),
        "run_command" => {
            run_command_from_workspace_with_progress(workspace_root, input, on_progress)
        }
        TEXT_EDITOR_TOOL_NAME => text_editor_from_workspace(workspace_root, input),
        other => Err(format!("Unsupported tool: {other}")),
    };
    let is_error = result.is_err();
    on_progress(AgentProgress::ToolCallFinished {
        name: name.to_string(),
        is_error,
    });

    match result {
        Ok(content) => ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content,
            is_error: None,
        },
        Err(message) => ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: message,
            is_error: Some(true),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use serde_json::json;
    use std::fs;

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
}
