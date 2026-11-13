use serde::Serialize;
use serde_json::Value;

pub(crate) mod apply_patch;
pub(crate) mod edit_history;
pub(crate) mod error;
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
use crate::tools::apply_patch::ApplyPatchTool;
use crate::tools::error::ToolError;
use crate::tools::find_symbol::FindSymbolTool;
use crate::tools::git::{GitBranchTool, GitCommitTool, GitDiffTool, GitLogTool, GitStatusTool};
use crate::tools::list_files::ListFilesTool;
use crate::tools::read_file::ReadFileTool;
use crate::tools::run_command::RunCommandTool;
use crate::tools::search_text::SearchTextTool;
use crate::tools::text_editor::TextEditorTool;
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

pub(crate) struct ToolContext<'a> {
    pub(crate) workspace_root: &'a Path,
    pub(crate) control: &'a crate::TaskControl,
}

/// A built-in tool: its wire schema and its execution, colocated so the
/// name/schema/implementation cannot drift apart.
pub(crate) trait Tool: Sync {
    fn name(&self) -> &'static str;
    fn definition(&self) -> ToolDefinition;
    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError>;
}

static BUILTIN_TOOLS: &[&dyn Tool] = &[
    &ReadFileTool,
    &ListFilesTool,
    &SearchTextTool,
    &ApplyPatchTool,
    &GitBranchTool,
    &GitStatusTool,
    &GitDiffTool,
    &GitLogTool,
    &GitCommitTool,
    &FindSymbolTool,
    &RunCommandTool,
    &TextEditorTool,
];

pub fn default_tools() -> Vec<ToolDefinition> {
    tools_for_model(true)
}

/// Tool definitions for a model. Without the native text editor, the text
/// editor is sent as an ordinary schema tool of the same name and input.
pub(crate) fn tools_for_model(native_text_editor: bool) -> Vec<ToolDefinition> {
    BUILTIN_TOOLS
        .iter()
        .map(|tool| match tool.definition() {
            ToolDefinition::AnthropicTextEditor(_) if !native_text_editor => {
                text_editor::portable_text_editor_definition()
            }
            definition => definition,
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn run_tool_calls(workspace_root: &Path, blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    let mut noop = |_| {};
    run_tool_calls_with_progress(workspace_root, blocks, &mut noop)
}

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn run_tool_call(
    workspace_root: &Path,
    tool_use_id: &str,
    name: &str,
    input: &Value,
) -> ContentBlock {
    let mut noop = |_| {};
    run_tool_call_with_progress(workspace_root, tool_use_id, name, input, &mut noop)
}

#[cfg(test)]
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
    run_tool_call_controlled(
        workspace_root,
        tool_use_id,
        name,
        input,
        &crate::TaskControl::default(),
        on_progress,
    )
}

#[cfg(test)]
pub(crate) fn run_tool_call_controlled<F>(
    workspace_root: &Path,
    tool_use_id: &str,
    name: &str,
    input: &Value,
    control: &crate::TaskControl,
    on_progress: &mut F,
) -> ContentBlock
where
    F: FnMut(AgentProgress),
{
    run_tool_call_checkpointed(
        workspace_root,
        tool_use_id,
        name,
        input,
        control,
        on_progress,
        &mut || Ok(()),
    )
}

pub(crate) fn run_tool_call_checkpointed<F>(
    workspace_root: &Path,
    tool_use_id: &str,
    name: &str,
    input: &Value,
    control: &crate::TaskControl,
    on_progress: &mut F,
    before_execute: &mut dyn FnMut() -> std::io::Result<()>,
) -> ContentBlock
where
    F: FnMut(AgentProgress),
{
    on_progress(AgentProgress::ToolCallStarted {
        name: name.to_string(),
    });
    let result = (|| -> Result<String, ToolError> {
        if control.is_cancelled() {
            return Err("Cancelled before execution".into());
        }
        if matches!(name, "run_command" | "git_commit") {
            let review = if name == "git_commit" {
                Some(git::commit_review(workspace_root, input)?)
            } else {
                None
            };
            let permissions = if name == "git_commit" {
                "Linux sandbox: workspace and Git metadata writable; agent state hidden; network denied"
            } else {
                "Linux sandbox: workspace writable; existing Git metadata read-only; agent state hidden; network denied"
            };
            let request = serde_json::json!({"workspace":workspace_root,"tool_call_id":tool_use_id,"arguments":input,"review":review,"permissions":permissions});
            if !control.approve(name, &request, on_progress) {
                return Err("Sandbox command denied: explicit human approval required; model approval/confirmed fields grant no authority".into());
            }
            if let Some(review) = review
                && git::commit_review(workspace_root, input)? != review
            {
                return Err(
                    "Repository changed during approval; commit refused, request a new review"
                        .into(),
                );
            }
        }
        before_execute().map_err(|e| {
            format!("Could not persist execution intent; tool was not started: {e}")
        })?;
        if control.is_cancelled() {
            return Err("Cancelled before execution; no side effects".into());
        }
        match BUILTIN_TOOLS.iter().find(|tool| tool.name() == name) {
            Some(tool) => tool.run(
                &ToolContext {
                    workspace_root,
                    control,
                },
                input,
                on_progress,
            ),
            None => Err(ToolError::UnsupportedTool(name.into())),
        }
    })();
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
        Err(err) => ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: err.to_string(),
            is_error: Some(true),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::TEXT_EDITOR_TOOL_NAME;
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

    #[test]
    fn models_without_the_native_editor_get_a_schema_tool_of_the_same_name() {
        let native = tools_for_model(true);
        let portable = tools_for_model(false);
        assert_eq!(native.len(), portable.len());
        assert!(
            portable
                .iter()
                .all(|tool| matches!(tool, ToolDefinition::Custom(_)))
        );
        let editor = portable
            .iter()
            .find_map(|tool| match tool {
                ToolDefinition::Custom(custom) if custom.name == TEXT_EDITOR_TOOL_NAME => {
                    Some(custom)
                }
                _ => None,
            })
            .expect("portable text editor");
        assert_eq!(editor.input_schema["required"], json!(["command", "path"]));
        assert!(editor.input_schema["properties"]["insert_text"].is_object());
    }
}
