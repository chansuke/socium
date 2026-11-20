//! The conversation as the agent, its providers and its saved state share
//! it: messages, their content blocks, the tools offered, and requests.
use crate::constants::DEFAULT_SYSTEM_PROMPT;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
pub(crate) struct AgentMessageRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    pub system: &'a str,
    #[serde(serialize_with = "wire_messages")]
    pub messages: &'a [Message],
    pub tools: &'a [ToolDefinition],
}

/// Messages as the API takes them: role and content, never Socium's own
/// bookkeeping such as `origin`.
fn wire_messages<S: serde::Serializer>(
    messages: &&[Message],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Wire<'a> {
        role: &'a Role,
        content: &'a MessageContent,
    }
    serializer.collect_seq(messages.iter().map(|message| Wire {
        role: &message.role,
        content: &message.content,
    }))
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

#[derive(Debug, Deserialize, Default)]
pub(crate) struct AgentMessageResponse {
    pub content: Vec<ContentBlock>,
    /// Why the response ended, in Anthropic's terms, when it ended in a way
    /// the turn has to handle: `max_tokens` for a reply cut at the output
    /// limit.
    #[serde(skip)]
    pub stop_reason: Option<String>,
    /// Tool calls whose input was not valid JSON, with the parse error.
    /// Their `ToolUse` blocks carry an empty object and must not run.
    #[serde(skip)]
    pub invalid_tool_inputs: Vec<(String, String)>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type")]
pub(crate) enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    /// Safety-redacted reasoning. `data` is opaque and encrypted; it is kept
    /// byte for byte and never shown.
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub(crate) struct Message {
    pub role: Role,
    pub content: MessageContent,
    /// Which provider and model wrote an assistant message. Its thinking is
    /// sent back only to a provider that can read it (ADR 14); `None` in
    /// history saved before this was recorded, and on user messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

/// The registry provider name and model ID that produced a message.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub(crate) struct Origin {
    pub provider: String,
    pub model: String,
}

impl Message {
    pub fn user_text(content: String) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Text(content),
            origin: None,
        }
    }

    pub fn assistant_blocks(content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::Assistant,
            content: MessageContent::Blocks(content),
            origin: None,
        }
    }

    /// An assistant message the given provider and model wrote.
    pub fn assistant_from(content: Vec<ContentBlock>, origin: Origin) -> Self {
        Self {
            origin: Some(origin),
            ..Self::assistant_blocks(content)
        }
    }

    pub fn user_blocks(content: Vec<ContentBlock>) -> Self {
        Self {
            role: Role::User,
            content: MessageContent::Blocks(content),
            origin: None,
        }
    }
}

impl ContentBlock {
    /// Thinking in either form, whose signature or data is bound to the
    /// model and the prefix it was produced after.
    pub(crate) fn is_thinking(&self) -> bool {
        matches!(
            self,
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
        )
    }
}

/// Removes thinking and redacted thinking blocks, whose signatures only the
/// model that produced them accepts, after the prefix they were produced
/// against. Returns how many were removed. An assistant message left empty
/// keeps a placeholder so the history stays well-formed.
pub(crate) fn strip_thinking(messages: &mut [Message]) -> usize {
    let mut removed = 0;
    for message in messages {
        let MessageContent::Blocks(blocks) = &mut message.content else {
            continue;
        };
        let before = blocks.len();
        blocks.retain(|block| !block.is_thinking());
        removed += before - blocks.len();
        if blocks.is_empty() && before > 0 {
            blocks.push(ContentBlock::Text {
                text: "(reasoning omitted)".to_string(),
            });
        }
    }
    removed
}

/// `messages` with the thinking a provider cannot read left out, or `None`
/// when it can read all of it. The saved history keeps every block, so a
/// later switch back to the model that wrote them sends them again. What is
/// left out depends only on the destination, so every block the current
/// model wrote was produced against this same projection and stays valid.
pub(crate) fn readable_thinking(
    messages: &[Message],
    reads: &dyn Fn(Option<&Origin>) -> bool,
) -> Option<Vec<Message>> {
    let unreadable = |message: &Message| {
        matches!(&message.content, MessageContent::Blocks(blocks) if blocks.iter().any(ContentBlock::is_thinking))
            && !reads(message.origin.as_ref())
    };
    if !messages.iter().any(unreadable) {
        return None;
    }
    let mut projected = messages.to_vec();
    for message in projected.iter_mut().filter(|message| unreadable(message)) {
        strip_thinking(std::slice::from_mut(message));
    }
    Some(projected)
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub(crate) enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    User,
    Assistant,
    System,
}

/// A tool as every provider is offered it: a name, a description, and a
/// JSON Schema for the input. A provider with a built-in form of a tool,
/// such as Anthropic's text editor, swaps it in when the request is built;
/// core never holds a provider's wire format.
#[derive(Debug, Serialize, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl ToolDefinition {
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Keeps the head and tail of a tool output within about `keep`
/// characters, saying how much was left out and why.
pub(crate) fn elide_tool_output(text: &str, keep: usize, why: &str) -> String {
    let length = text.chars().count();
    if length <= keep {
        return text.to_string();
    }
    let head = keep / 2;
    let tail = keep - head;
    let start = text.chars().take(head).collect::<String>();
    let end = text.chars().skip(length - tail).collect::<String>();
    format!(
        "{start}\n[... {} characters omitted {why}; ask for a narrower range to see them ...]\n{end}",
        length - keep
    )
}

#[cfg(test)]
mod tests {
    use super::{AgentMessageRequest, ContentBlock, Message, MessageContent, strip_thinking};
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

    #[test]
    fn strip_thinking_keeps_other_blocks_and_never_leaves_a_message_empty() {
        let thinking = || ContentBlock::Thinking {
            thinking: "plan".into(),
            signature: "sig".into(),
        };
        let mut messages = vec![
            Message::user_text("hi".into()),
            Message::assistant_blocks(vec![
                thinking(),
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ]),
            Message::assistant_blocks(vec![
                thinking(),
                ContentBlock::RedactedThinking {
                    data: "opaque".into(),
                },
            ]),
        ];

        assert_eq!(strip_thinking(&mut messages), 3);
        let blocks = |message: &Message| match &message.content {
            MessageContent::Blocks(blocks) => blocks.clone(),
            MessageContent::Text(_) => panic!("expected blocks"),
        };
        assert_eq!(
            blocks(&messages[1]),
            [ContentBlock::Text {
                text: "answer".into()
            }]
        );
        assert_eq!(
            blocks(&messages[2]),
            [ContentBlock::Text {
                text: "(reasoning omitted)".into()
            }]
        );
        assert_eq!(strip_thinking(&mut messages), 0);
    }

    #[test]
    fn redacted_thinking_round_trips_its_opaque_data_unchanged() {
        let data = "EmwKAhgBEgy3va3pzix/LafPsn4aDFIT2Xlxh0L5L8rLVyIwxtE3rAFBa8cr3qpP";
        let wire = serde_json::json!({"type": "redacted_thinking", "data": data});

        let block: ContentBlock = serde_json::from_value(wire.clone()).unwrap();

        assert_eq!(block, ContentBlock::RedactedThinking { data: data.into() });
        assert!(block.is_thinking());
        assert_eq!(serde_json::to_value(&block).unwrap(), wire);
    }
}
