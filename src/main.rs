use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{self, BufRead, Error, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const ANTHROPIC_MESSAGES_API: &str = "https://api.anthropic.com/v1/messages";
const MAX_READ_FILE_BYTES: usize = 200 * 1024;

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
struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

fn read_file_tool() -> ToolDefinition {
    ToolDefinition {
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
    }
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

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    let Some(user_input) = parse_input(">> ")? else {
        return Ok(());
    };

    let mut messages = vec![Message::user_text(user_input)];
    let tools = vec![read_file_tool()];
    let workspace_root = env::current_dir()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    loop {
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
            break;
        }
        messages.push(Message::user_blocks(tool_results));
    }

    Ok(())
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
            ContentBlock::ToolUse { id, name, input } => Some(run_tool_call(
                workspace_root,
                id,
                name,
                input,
            )),
            _ => None,
        })
        .collect()
}

fn run_tool_call(workspace_root: &Path, tool_use_id: &str, name: &str, input: &Value) -> ContentBlock {
    let result = match name {
        "read_file" => read_file_from_workspace(workspace_root, input),
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
    let mut file =
        fs::File::open(&resolved_path).map_err(|e| format!("Failed to read file '{}': {e}", path))?;
    let mut bytes = Vec::new();
    let mut limited = Read::by_ref(&mut file).take((MAX_READ_FILE_BYTES + 1) as u64);
    limited
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Failed to read file '{}': {e}", path))?;
    if bytes.len() > MAX_READ_FILE_BYTES {
        return Err(format!(
            "File too large: '{}' exceeds {} bytes",
            path, MAX_READ_FILE_BYTES
        ));
    }
    let content =
        String::from_utf8(bytes).map_err(|e| format!("File '{}' is not valid UTF-8: {e}", path))?;
    Ok(format!("Path: {}\n{}", path, content))
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
        return Err(format!("Absolute paths are not allowed: '{}'", requested_path));
    }
    let joined = workspace_canonical.join(path);
    let canonical = joined
        .canonicalize()
        .map_err(|e| format!("Failed to resolve path '{}': {e}", joined.display()))?;
    if !canonical.starts_with(&workspace_canonical) {
        return Err(format!(
            "Path '{}' is outside the workspace '{}'",
            canonical.display(),
            workspace_canonical.display()
        ));
    }
    Ok(canonical)
}

/// Prompts once-per-line until a non-empty input is entered.
/// Returns `Ok(None)` on EOF (e.g. Ctrl-D), so callers can exit gracefully.
pub fn parse_input(prompt: &str) -> io::Result<Option<String>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();

    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut err_writer = stderr.lock();

    parse_input_from(prompt, &mut reader, &mut writer, &mut err_writer)
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
    use super::{MAX_READ_FILE_BYTES, parse_input_from, read_file_from_workspace};
    use serde_json::json;
    use std::fs;
    use std::io::Cursor;
    use std::path::PathBuf;
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
        let file_path = workspace.join("note.txt");
        fs::write(&file_path, "hello").unwrap();

        let output = read_file_from_workspace(&workspace, &json!({"path": "note.txt"})).unwrap();

        assert!(output.contains("Path:"));
        assert!(output.contains("hello"));

        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn rejects_path_outside_workspace() {
        let workspace = make_temp_dir("workspace");
        let outside = make_temp_dir("outside");
        let file_path = outside.join("secret.txt");
        fs::write(&file_path, "secret").unwrap();

        let output = read_file_from_workspace(
            &workspace,
            &json!({"path": file_path.to_string_lossy().to_string()}),
        );

        assert!(output.is_err());

        fs::remove_dir_all(workspace).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn rejects_too_large_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.join("large.txt");
        let large = vec![b'a'; MAX_READ_FILE_BYTES + 1];
        fs::write(&file_path, large).unwrap();

        let output = read_file_from_workspace(&workspace, &json!({"path": "large.txt"}));

        assert!(output.is_err());

        fs::remove_dir_all(workspace).unwrap();
    }

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("socium_{prefix}_{nanos}"));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
