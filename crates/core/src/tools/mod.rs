use serde::Serialize;
use serde_json::{Value, json};

use crate::constants::TEXT_EDITOR_TOOL_NAME;
use crate::constants::{
    MAX_COMMAND_TIMEOUT_MS, MAX_SEARCH_RESULTS, MAX_TEXT_EDITOR_VIEW_CHARACTERS,
};

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
        find_symbol_tool(),
        run_command_tool(),
        text_editor_tool(),
    ]
}
