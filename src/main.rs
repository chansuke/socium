use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{self, BufRead, Error, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const ANTHROPIC_MESSAGES_API: &str = "https://api.anthropic.com/v1/messages";
const MAX_READ_FILE_BYTES: usize = 200 * 1024;
const MAX_TOOL_ROUNDS: usize = 20;
const MAX_LIST_ENTRIES: usize = 1000;
const TEXT_EDITOR_TOOL_NAME: &str = "str_replace_based_edit_tool";
// Keep this aligned with Anthropic text editor tool output limits.
const MAX_TEXT_EDITOR_VIEW_CHARACTERS: usize = 50_000;
const MAX_TEXT_EDITOR_VIEW_READ_BYTES: usize = 4 * 1024 * 1024;
const STATE_DIR_NAME: &str = ".socium";
const CONVERSATION_STATE_FILE_NAME: &str = "conversation.json";
const INPUT_HISTORY_FILE_NAME: &str = "input_history.json";
const MAX_INPUT_HISTORY_ENTRIES: usize = 1_000;

#[derive(Debug, Serialize)]
struct AgentMessageRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    pub messages: &'a [Message],
    pub tools: &'a [ToolDefinition],
}

impl<'a> AgentMessageRequest<'a> {
    pub fn new(
        model: &'a str,
        max_tokens: u32,
        messages: &'a [Message],
        tools: &'a [ToolDefinition],
    ) -> Self {
        Self {
            model,
            max_tokens,
            messages,
            tools,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AgentMessageResponse {
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(untagged)]
enum ToolDefinition {
    Custom(CustomToolDefinition),
    AnthropicTextEditor(AnthropicTextEditorToolDefinition),
}

#[derive(Debug, Serialize, Clone)]
struct CustomToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Serialize, Clone)]
struct AnthropicTextEditorToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_characters: Option<usize>,
}

fn read_file_tool() -> ToolDefinition {
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

fn list_files_tool() -> ToolDefinition {
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

fn text_editor_tool() -> ToolDefinition {
    ToolDefinition::AnthropicTextEditor(AnthropicTextEditorToolDefinition {
        tool_type: "text_editor_20250728".to_string(),
        name: TEXT_EDITOR_TOOL_NAME.to_string(),
        max_characters: Some(MAX_TEXT_EDITOR_VIEW_CHARACTERS),
    })
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Message {
    pub role: Role,
    pub content: MessageContent,
}

impl Message {
    pub fn user_text(content: String) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Text(content),
        }
    }

    pub fn assistant_blocks(content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Blocks(content),
        }
    }

    pub fn user_blocks(content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Blocks(content),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct ConversationState {
    messages: Vec<Message>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct InputHistoryState {
    entries: Vec<String>,
}

struct InputSession {
    history_path: PathBuf,
    entries: Vec<String>,
}

impl InputSession {
    fn load(history_path: PathBuf) -> io::Result<Self> {
        let state = load_json_file::<InputHistoryState>(&history_path)?;
        Ok(Self {
            history_path,
            entries: state.entries,
        })
    }

    fn record_entry(&mut self, entry: &str) -> io::Result<()> {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        if self.entries.last().is_some_and(|last| last == entry) {
            return Ok(());
        }

        self.entries.push(entry.to_string());
        if self.entries.len() > MAX_INPUT_HISTORY_ENTRIES {
            let overflow = self.entries.len() - MAX_INPUT_HISTORY_ENTRIES;
            self.entries.drain(0..overflow);
        }

        save_json_file(
            &self.history_path,
            &InputHistoryState {
                entries: self.entries.clone(),
            },
        )
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = env::current_dir()?;
    let state_dir = workspace_root.join(STATE_DIR_NAME);
    let conversation_path = state_dir.join(CONVERSATION_STATE_FILE_NAME);

    let mut messages = match load_json_file::<ConversationState>(&conversation_path) {
        Ok(state) => state.messages,
        Err(err) => {
            eprintln!(
                "Failed to load persisted conversation '{}': {err}. Starting with an empty conversation.",
                conversation_path.display()
            );
            Vec::new()
        }
    };
    let mut input_session = match InputSession::load(state_dir.join(INPUT_HISTORY_FILE_NAME)) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("Failed to load input history: {err}. Starting with empty input history.");
            InputSession {
                history_path: state_dir.join(INPUT_HISTORY_FILE_NAME),
                entries: Vec::new(),
            }
        }
    };

    let api_key = env::var("ANTHROPIC_API_KEY")
        .map_err(|_| Error::new(ErrorKind::NotFound, "ANTHROPIC_API_KEY is not set"))?;
    let anthropic_version =
        env::var("ANTHROPIC_VERSION").unwrap_or_else(|_| "2023-06-01".to_string());
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

    let Some(user_input) = parse_input(">> ", &mut input_session)? else {
        return Ok(());
    };

    messages.push(Message::user_text(user_input));
    let tools = vec![read_file_tool(), list_files_tool(), text_editor_tool()];
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    let mut finished = false;
    for _ in 0..MAX_TOOL_ROUNDS {
        let message_request = AgentMessageRequest::new(&model, max_tokens, &messages, &tools);
        let response =
            send_message(&client, &api_key, &anthropic_version, &message_request).await?;
        let assistant_blocks = response.content;

        let message_text = extract_text(&assistant_blocks);
        if !message_text.is_empty() {
            println!("{message_text}");
        }

        let tool_results = run_tool_calls(&workspace_root, &assistant_blocks);
        messages.push(Message::assistant_blocks(assistant_blocks));
        if tool_results.is_empty() {
            finished = true;
            break;
        }
        messages.push(Message::user_blocks(tool_results));
    }

    if !finished {
        return Err(Error::other(format!(
            "Tool execution reached the maximum rounds ({MAX_TOOL_ROUNDS}) and may be incomplete"
        ))
        .into());
    }

    save_json_file(
        &conversation_path,
        &ConversationState {
            messages: messages.clone(),
        },
    )?;

    Ok(())
}

fn load_json_file<T>(path: &Path) -> io::Result<T>
where
    T: for<'de> Deserialize<'de> + Default,
{
    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content).map_err(|err| {
            Error::new(
                ErrorKind::InvalidData,
                format!("Invalid JSON in '{}': {err}", path.display()),
            )
        }),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(T::default()),
        Err(err) => Err(err),
    }
}

fn save_json_file<T>(path: &Path, value: &T) -> io::Result<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_string_pretty(value).map_err(|err| {
        Error::other(format!(
            "Failed to serialize JSON '{}': {err}",
            path.display()
        ))
    })?;
    fs::write(path, serialized)
}

async fn send_message(
    client: &reqwest::Client,
    api_key: &str,
    anthropic_version: &str,
    message_request: &AgentMessageRequest<'_>,
) -> Result<AgentMessageResponse, Box<dyn std::error::Error>> {
    let response = client
        .post(ANTHROPIC_MESSAGES_API)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", anthropic_version)
        .json(message_request)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await?;
        return Err(Error::other(format!("Anthropic API error {status}: {error_body}")).into());
    }
    Ok(response.json().await?)
}

fn extract_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_tool_calls(workspace_root: &Path, blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => {
                Some(run_tool_call(workspace_root, id, name, input))
            }
            _ => None,
        })
        .collect()
}

fn run_tool_call(
    workspace_root: &Path,
    tool_use_id: &str,
    name: &str,
    input: &Value,
) -> ContentBlock {
    let result = match name {
        "read_file" => read_file_from_workspace(workspace_root, input),
        "list_files" => list_files_from_workspace(workspace_root, input),
        TEXT_EDITOR_TOOL_NAME => text_editor_from_workspace(workspace_root, input),
        other => Err(format!("Unsupported tool: {other}")),
    };

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

fn text_editor_from_workspace(workspace_root: &Path, input: &Value) -> Result<String, String> {
    let input_object = input
        .as_object()
        .ok_or("text_editor tool requires an object input")?;
    let command = require_string_field(input_object, "command", "text_editor command")?;

    match command {
        "view" => text_editor_view(workspace_root, input_object),
        "str_replace" => text_editor_str_replace(workspace_root, input_object),
        "create" => text_editor_create(workspace_root, input_object),
        "insert" => text_editor_insert(workspace_root, input_object),
        "undo_edit" => {
            Err("undo_edit is not supported by this text editor implementation".to_string())
        }
        other => Err(format!("Unsupported text editor command: {other}")),
    }
}

fn text_editor_view(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, String> {
    let path = require_string_field(input, "path", "text_editor view path")?;
    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect path '{}': {e}", path))?;

    if metadata.is_dir() {
        return list_files_from_workspace(workspace_root, &json!({ "path": path }));
    }
    if !metadata.is_file() {
        return Err(format!("Path is neither a file nor directory: '{}'", path));
    }

    let content = read_utf8_file_with_limit(&resolved_path, path, MAX_TEXT_EDITOR_VIEW_READ_BYTES)?;
    let selected_content = apply_view_range(&content, input.get("view_range"))?;
    let (display_content, truncated) =
        truncate_by_characters(&selected_content, MAX_TEXT_EDITOR_VIEW_CHARACTERS);

    if truncated {
        Ok(format!(
            "Path: {}\n{}\n(truncated: showing first {} characters)",
            path, display_content, MAX_TEXT_EDITOR_VIEW_CHARACTERS
        ))
    } else {
        Ok(format!("Path: {}\n{}", path, display_content))
    }
}

fn text_editor_str_replace(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, String> {
    let path = require_string_field(input, "path", "text_editor str_replace path")?;
    let old_str = require_string_field(input, "old_str", "text_editor str_replace old_str")?;
    let new_str = require_string_field(input, "new_str", "text_editor str_replace new_str")?;
    if old_str.is_empty() {
        return Err("text_editor str_replace old_str cannot be empty".to_string());
    }

    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", path))?;
    if !metadata.is_file() {
        return Err(format!("Path is not a file: '{}'", path));
    }

    let content = read_utf8_file_with_limit(&resolved_path, path, MAX_READ_FILE_BYTES)?;
    let matches = content.match_indices(old_str).count();
    if matches == 0 {
        return Err(format!(
            "No matches found for old_str in file '{}'; replacement not applied",
            path
        ));
    }
    if matches > 1 {
        return Err(format!(
            "Multiple matches found for old_str in file '{}'; provide a more specific old_str",
            path
        ));
    }

    let updated = content.replacen(old_str, new_str, 1);
    fs::write(&resolved_path, updated)
        .map_err(|e| format!("Failed to write file '{}': {e}", path))?;
    Ok(format!("Path: {}\nReplaced 1 occurrence", path))
}

fn text_editor_create(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, String> {
    let path = require_string_field(input, "path", "text_editor create path")?;
    let file_text = require_string_field(input, "file_text", "text_editor create file_text")?;
    let target_path = resolve_workspace_path_for_create(workspace_root, path)?;

    if target_path.exists() {
        return Err(format!(
            "Cannot create file '{}': path already exists",
            path
        ));
    }
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directories for '{}': {e}", path))?;
    }
    fs::write(&target_path, file_text)
        .map_err(|e| format!("Failed to create file '{}': {e}", path))?;
    Ok(format!("Path: {}\nCreated file", path))
}

fn text_editor_insert(
    workspace_root: &Path,
    input: &serde_json::Map<String, Value>,
) -> Result<String, String> {
    let path = require_string_field(input, "path", "text_editor insert path")?;
    let insert_line = require_i64_field(input, "insert_line", "text_editor insert insert_line")?;
    if insert_line < 0 {
        return Err("text_editor insert insert_line must be >= 0".to_string());
    }

    let insert_text = require_string_field(input, "insert_text", "text_editor insert insert_text")?;

    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", path))?;
    if !metadata.is_file() {
        return Err(format!("Path is not a file: '{}'", path));
    }

    let content = read_utf8_file_with_limit(&resolved_path, path, MAX_READ_FILE_BYTES)?;
    let line_count = count_lines(&content);
    let line = insert_line as usize;
    if line > line_count {
        return Err(format!(
            "insert_line {} is out of range for '{}' (file has {} lines)",
            line, path, line_count
        ));
    }

    let insert_at = byte_index_after_line(&content, line).ok_or_else(|| {
        format!(
            "Failed to compute insertion point for line {} in '{}'",
            line, path
        )
    })?;
    let mut updated = String::with_capacity(content.len() + insert_text.len());
    updated.push_str(&content[..insert_at]);
    updated.push_str(insert_text);
    updated.push_str(&content[insert_at..]);

    fs::write(&resolved_path, updated)
        .map_err(|e| format!("Failed to write file '{}': {e}", path))?;
    Ok(format!("Path: {}\nInserted text after line {}", path, line))
}

fn require_string_field<'a>(
    input: &'a serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<&'a str, String> {
    input
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context} requires a string field: {field}"))
}

fn require_i64_field(
    input: &serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<i64, String> {
    input
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("{context} requires an integer field: {field}"))
}

fn apply_view_range(content: &str, view_range: Option<&Value>) -> Result<String, String> {
    let Some(view_range) = view_range else {
        return Ok(content.to_string());
    };
    let range = view_range
        .as_array()
        .ok_or("text_editor view_range must be an array: [start_line, end_line]")?;
    if range.len() != 2 {
        return Err("text_editor view_range must contain exactly two integers".to_string());
    }
    let start = range[0]
        .as_i64()
        .ok_or("text_editor view_range start must be an integer")?;
    let end = range[1]
        .as_i64()
        .ok_or("text_editor view_range end must be an integer")?;
    if start < 1 {
        return Err("text_editor view_range start must be >= 1".to_string());
    }
    if end < -1 || end == 0 {
        return Err("text_editor view_range end must be -1 or a positive integer".to_string());
    }
    if end != -1 && end < start {
        return Err("text_editor view_range end must be -1 or >= start".to_string());
    }

    let line_count = count_lines(content) as i64;
    if line_count == 0 {
        if start == 1 {
            return Ok(String::new());
        }
        return Err("text_editor view_range start is out of range for an empty file".to_string());
    }
    if start > line_count {
        return Err(format!(
            "text_editor view_range start {} is out of range (file has {} lines)",
            start, line_count
        ));
    }
    let last = if end == -1 {
        line_count
    } else {
        std::cmp::min(end, line_count)
    };
    let start_line = start as usize;
    let end_line = last as usize;

    let start_offset = line_start_offset(content, start_line).ok_or_else(|| {
        format!(
            "text_editor view_range start {} is out of range (file has {} lines)",
            start, line_count
        )
    })?;
    let end_offset = if end_line as i64 == line_count {
        content.len()
    } else {
        line_start_offset(content, end_line + 1).ok_or_else(|| {
            format!(
                "text_editor view_range end {} is out of range (file has {} lines)",
                end_line, line_count
            )
        })?
    };

    Ok(content[start_offset..end_offset].to_string())
}

fn truncate_by_characters(content: &str, max_characters: usize) -> (String, bool) {
    if max_characters == 0 {
        return (String::new(), !content.is_empty());
    }

    let mut end = 0usize;
    for (count, (index, ch)) in content.char_indices().enumerate() {
        if count == max_characters {
            return (content[..end].to_string(), true);
        }
        end = index + ch.len_utf8();
    }
    (content.to_string(), false)
}

fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count()
    }
}

fn byte_index_after_line(content: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return Some(0);
    }
    let mut seen = 0usize;
    for (index, ch) in content.char_indices() {
        if ch == '\n' {
            seen += 1;
            if seen == line {
                return Some(index + 1);
            }
        }
    }
    if line == count_lines(content) {
        return Some(content.len());
    }
    None
}

fn line_start_offset(content: &str, line: usize) -> Option<usize> {
    if line == 0 {
        return None;
    }
    if line == 1 {
        return Some(0);
    }

    let mut current_line = 1usize;
    for (index, byte) in content.as_bytes().iter().enumerate() {
        if *byte == b'\n' {
            current_line += 1;
            if current_line == line {
                return Some(index + 1);
            }
        }
    }
    None
}

fn read_utf8_file_with_limit(
    path: &Path,
    display_path: &str,
    max_bytes: usize,
) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("Failed to open file '{}': {e}", display_path))?;
    let mut bytes = Vec::new();
    let mut limited = Read::by_ref(&mut file).take((max_bytes + 1) as u64);
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read file '{}': {e}", display_path))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "File too large: '{}' exceeds {} bytes",
            display_path, max_bytes
        ));
    }
    String::from_utf8(bytes).map_err(|e| format!("File '{}' is not valid UTF-8: {e}", display_path))
}

fn read_file_from_workspace(workspace_root: &Path, input: &Value) -> Result<String, String> {
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .ok_or("read_file requires a string field: path")?;
    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect file '{}': {e}", path))?;
    if !metadata.is_file() {
        return Err(format!("Path is not a file: '{}'", path));
    }
    let content = read_utf8_file_with_limit(&resolved_path, path, MAX_READ_FILE_BYTES)?;
    Ok(format!("Path: {}\n{}", path, content))
}

fn list_files_from_workspace(workspace_root: &Path, input: &Value) -> Result<String, String> {
    let input_object = input
        .as_object()
        .ok_or("list_files requires an object input")?;
    let raw_path = match input_object.get("path") {
        None => ".",
        Some(value) => value
            .as_str()
            .ok_or("list_files requires an optional string field: path")?,
    };
    let path = if raw_path.is_empty() { "." } else { raw_path };

    let resolved_path = resolve_workspace_path(workspace_root, path)?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|e| format!("Failed to inspect path '{}': {e}", path))?;

    if metadata.is_file() {
        return Ok(format!("Path: {}\n[file]", path));
    }

    if !metadata.is_dir() {
        return Err(format!("Path is neither a file nor directory: '{}'", path));
    }

    let read_dir = fs::read_dir(&resolved_path)
        .map_err(|e| format!("Failed to list directory '{}': {e}", path))?;
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut has_more = false;

    for entry_result in read_dir {
        let entry = entry_result.map_err(|e| format!("Failed to read directory entry: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to inspect directory entry type: {e}"))?;
        let (prefix, display_name) = if file_type.is_symlink() {
            ("[lnk]", name.clone())
        } else if file_type.is_dir() {
            ("[dir]", format!("{name}/"))
        } else {
            ("[file]", name.clone())
        };
        entries.push((name, format!("{prefix} {display_name}")));

        if entries.len() > MAX_LIST_ENTRIES {
            has_more = true;
            break;
        }
    }

    if entries.is_empty() {
        return Ok(format!("Path: {}\n(empty)", path));
    }

    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    if has_more {
        let lines: Vec<String> = entries
            .iter()
            .take(MAX_LIST_ENTRIES)
            .map(|(_, line)| line.clone())
            .collect();
        Ok(format!(
            "Path: {}\n{}\n(truncated: showing first {MAX_LIST_ENTRIES} entries; more entries omitted)",
            path,
            lines.join("\n")
        ))
    } else {
        let lines: Vec<String> = entries.into_iter().map(|(_, line)| line).collect();
        Ok(format!("Path: {}\n{}", path, lines.join("\n")))
    }
}

fn resolve_workspace_path(workspace_root: &Path, requested_path: &str) -> Result<PathBuf, String> {
    let workspace_canonical = workspace_root.canonicalize().map_err(|e| {
        format!(
            "Failed to resolve workspace root '{}': {e}",
            workspace_root.display()
        )
    })?;
    let path = PathBuf::from(requested_path);
    if path.is_absolute() {
        return Err(format!(
            "Absolute paths are not allowed: '{}'",
            requested_path
        ));
    }
    let joined = workspace_canonical.join(path);
    let canonical = joined
        .canonicalize()
        .map_err(|e| format!("Failed to resolve path '{}': {e}", requested_path))?;
    if !canonical.starts_with(&workspace_canonical) {
        return Err(format!(
            "Path '{}' is outside the workspace",
            requested_path
        ));
    }
    Ok(canonical)
}

fn resolve_workspace_path_for_create(
    workspace_root: &Path,
    requested_path: &str,
) -> Result<PathBuf, String> {
    let workspace_canonical = workspace_root.canonicalize().map_err(|e| {
        format!(
            "Failed to resolve workspace root '{}': {e}",
            workspace_root.display()
        )
    })?;
    let path = PathBuf::from(requested_path);
    if path.is_absolute() {
        return Err(format!(
            "Absolute paths are not allowed: '{}'",
            requested_path
        ));
    }
    let joined = workspace_canonical.join(path);
    match fs::symlink_metadata(&joined) {
        Ok(metadata) => {
            let canonical = match joined.canonicalize() {
                Ok(path) => path,
                Err(e) if metadata.file_type().is_symlink() && e.kind() == ErrorKind::NotFound => {
                    return Err(format!(
                        "Path '{}' points to a dangling symlink",
                        requested_path
                    ));
                }
                Err(e) => return Err(format!("Failed to resolve path '{}': {e}", requested_path)),
            };
            if !canonical.starts_with(&workspace_canonical) {
                return Err(format!(
                    "Path '{}' is outside the workspace",
                    requested_path
                ));
            }
            return Ok(canonical);
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!("Failed to inspect path '{}': {e}", requested_path));
        }
    }

    let mut canonical_ancestor: Option<PathBuf> = None;
    for ancestor in joined.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                let canonical = match ancestor.canonicalize() {
                    Ok(path) => path,
                    Err(e)
                        if metadata.file_type().is_symlink() && e.kind() == ErrorKind::NotFound =>
                    {
                        return Err(format!(
                            "Path '{}' includes a dangling symlink in an ancestor",
                            requested_path
                        ));
                    }
                    Err(e) => {
                        return Err(format!(
                            "Failed to resolve parent directory for '{}': {e}",
                            requested_path
                        ));
                    }
                };
                canonical_ancestor = Some(canonical);
                break;
            }
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to inspect parent directory for '{}': {e}",
                    requested_path
                ));
            }
        }
    }

    let canonical_ancestor = canonical_ancestor.ok_or_else(|| {
        format!(
            "Failed to resolve parent directory for path '{}'",
            requested_path
        )
    })?;
    if !canonical_ancestor.starts_with(&workspace_canonical) {
        return Err(format!(
            "Path '{}' is outside the workspace",
            requested_path
        ));
    }
    Ok(joined)
}

/// Prompts once-per-line until a non-empty input is entered.
/// Returns `Ok(None)` on EOF (e.g. Ctrl-D), so callers can exit gracefully.
fn parse_input(prompt: &str, input_session: &mut InputSession) -> io::Result<Option<String>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut err_writer = stderr.lock();
    let input = parse_input_from(prompt, &mut reader, &mut writer, &mut err_writer)?;
    if let Some(entry) = input.as_deref() {
        input_session.record_entry(entry)?;
    }
    Ok(input)
}

fn parse_input_from<R, W, E>(
    prompt: &str,
    reader: &mut R,
    writer: &mut W,
    err_writer: &mut E,
) -> io::Result<Option<String>>
where
    R: BufRead,
    W: Write,
    E: Write,
{
    let mut input = String::new();

    loop {
        write!(writer, "{prompt}")?;
        writer.flush()?;

        input.clear();
        let bytes_read = reader.read_line(&mut input)?;
        if bytes_read == 0 {
            writeln!(writer)?;
            return Ok(None);
        }

        if input.trim().is_empty() {
            writeln!(err_writer, "Input cannot be empty. Please try again.")?;
            continue;
        }

        return Ok(Some(input.trim_end().to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ContentBlock, MAX_LIST_ENTRIES, MAX_READ_FILE_BYTES, MAX_TEXT_EDITOR_VIEW_READ_BYTES,
        TEXT_EDITOR_TOOL_NAME, extract_text, list_files_from_workspace, parse_input_from,
        read_file_from_workspace, resolve_workspace_path, resolve_workspace_path_for_create,
        run_tool_call, run_tool_calls,
    };
    use serde_json::json;
    use std::fs;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn returns_trimmed_input() {
        let mut reader = Cursor::new(b"hello\n");
        let mut out = Vec::new();
        let mut err = Vec::new();

        let input = parse_input_from(">> ", &mut reader, &mut out, &mut err).unwrap();

        assert_eq!(input, Some("hello".to_string()));
        assert_eq!(String::from_utf8(out).unwrap(), ">> ");
        assert!(err.is_empty());
    }

    #[test]
    fn prompts_again_on_empty_input() {
        let mut reader = Cursor::new(b"\n  \nfinal input\n");
        let mut out = Vec::new();
        let mut err = Vec::new();

        let input = parse_input_from(">> ", &mut reader, &mut out, &mut err).unwrap();

        assert_eq!(input, Some("final input".to_string()));
        assert_eq!(String::from_utf8(out).unwrap(), ">> >> >> ");
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "Input cannot be empty. Please try again.\nInput cannot be empty. Please try again.\n"
        );
    }

    #[test]
    fn returns_none_on_eof() {
        let mut reader = Cursor::new(Vec::<u8>::new());
        let mut out = Vec::new();
        let mut err = Vec::new();

        let input = parse_input_from(">> ", &mut reader, &mut out, &mut err).unwrap();

        assert_eq!(input, None);
        assert_eq!(String::from_utf8(out).unwrap(), ">> \n");
        assert!(err.is_empty());
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
    fn text_editor_undo_edit_returns_error() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("undo.txt"), "x").unwrap();

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
                assert!(content.contains("undo_edit is not supported"));
                assert_eq!(is_error, Some(true));
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
}
