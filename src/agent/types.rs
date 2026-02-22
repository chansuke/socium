use crate::tools::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
pub(crate) struct AgentMessageRequest<'a> {
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
pub(crate) struct AgentMessageResponse {
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub(crate) enum ContentBlock {
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
pub(crate) struct Message {
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
pub(crate) enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    User,
    Assistant,
    System,
}
