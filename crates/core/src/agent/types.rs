use crate::constants::DEFAULT_SYSTEM_PROMPT;
use crate::tools::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
pub(crate) struct AgentMessageRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    pub system: &'a str,
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
        Self::with_system_prompt(model, max_tokens, DEFAULT_SYSTEM_PROMPT, messages, tools)
    }

    pub fn with_system_prompt(
        model: &'a str,
        max_tokens: u32,
        system: &'a str,
        messages: &'a [Message],
        tools: &'a [ToolDefinition],
    ) -> Self {
        Self {
            model,
            max_tokens,
            system,
            messages,
            tools,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct AgentMessageResponse {
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type")]
pub(crate) enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
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

#[cfg(test)]
mod tests {
    use super::{AgentMessageRequest, Message};
    use crate::constants::DEFAULT_SYSTEM_PROMPT;
    use crate::tools::default_tools;

    #[test]
    fn agent_message_request_serializes_default_system_prompt() {
        let messages = vec![Message::user_text("hello".to_string())];
        let tools = default_tools();
        let request = AgentMessageRequest::new("test-model", 128, &messages, &tools);

        let value = serde_json::to_value(&request).unwrap();

        assert_eq!(value["system"], DEFAULT_SYSTEM_PROMPT);
        assert!(value["system"].as_str().unwrap().contains("path:line"));
    }
}
