use serde::{Deserialize, Serialize};
use std::env;
use std::io::{self, BufRead, Error, ErrorKind, Write};
use std::time::Duration;

#[derive(Debug, Serialize, Deserialize)]
struct AgentMessageRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<Message>,
}

impl AgentMessageRequest {
    pub fn new(model: String, max_tokens: u32, messages: Vec<Message>) -> Self {
        Self {
            model,
            max_tokens,
            messages,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentMessageResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub role: Role,
    pub content: Vec<Content>,
    pub model: String,
    pub stop_reason: String,
    pub usage: Usage,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum Content {
    #[serde(rename = "text")]
    Text { text: String },
}

#[derive(Debug, Serialize, Deserialize)]
struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn new(role: Role, content: String) -> Self {
        Self { role, content }
    }
}

#[derive(Debug, Serialize, Deserialize)]
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
    let message = Message::new(Role::User, user_input);
    let messages = vec![message];
    let message_request = AgentMessageRequest::new(model, max_tokens, messages);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", anthropic_version)
        .json(&message_request)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let error_body = response.text().await?;
        return Err(Error::new(
            ErrorKind::Other,
            format!("Anthropic API error {status}: {error_body}"),
        )
        .into());
    }
    let new_response: AgentMessageResponse = response.json().await?;
    let message_text = new_response
        .content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text } => Some(text.as_str()),
        })
        .collect::<Vec<_>>()
        .join("\n");
    println!("\n{}", message_text);

    Ok(())
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
    use super::parse_input_from;
    use std::io::Cursor;

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
}
