use serde::{Deserialize, Serialize};

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
pub enum Role {
    User,
    Assistant,
    System,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = "".to_string();
    let max_tokens = 1000;
    let message = Message::new(Role::User, "".to_string());
    let messages = vec![message];
    let message_request = AgentMessageRequest::new(model, max_tokens, messages);
    let new_response: AgentMessageResponse = reqwest::Client::new()
        .post("https://api.anthropic.com/v1/messages")
        .json(&message_request)
        .send()
        .await?
        .json()
        .await?;

    println!("{new_response:#?}");

    Ok(())
}
