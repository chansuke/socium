use serde::Serialize;
use serde_json::{Value, json};

use crate::constants::TEXT_EDITOR_TOOL_NAME;
use crate::constants::{MAX_COMMAND_TIMEOUT_MS, MAX_TEXT_EDITOR_VIEW_CHARACTERS};

pub mod runtime;

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
        description: "List files and directories at a given path. If no path is provided, lists files in the current directory. If a file path is given, confirms the file exists."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional relative path to a file or directory in the workspace. Defaults to the current directory."
                }
            },
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
        run_command_tool(),
        text_editor_tool(),
    ]
}
