use crate::agent::types::{AgentMessageRequest, ContentBlock, Message};
use crate::constants::MAX_TOOL_ROUNDS;
use crate::providers;
use crate::providers::{ProviderConfig, ProviderKind};
use crate::tools::ToolDefinition;
use crate::tools::runtime::run_tool_calls;
use std::io::Error;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_agent_turn(
    provider: ProviderKind,
    client: &reqwest::Client,
    provider_config: &ProviderConfig,
    model: &str,
    max_tokens: u32,
    supports_tools: bool,
    tools: &[ToolDefinition],
    workspace_root: &Path,
    messages: &mut Vec<Message>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut finished = false;
    let max_rounds = if supports_tools { MAX_TOOL_ROUNDS } else { 1 };
    for _ in 0..max_rounds {
        let message_request = AgentMessageRequest::new(model, max_tokens, messages, tools);
        let response =
            providers::send_message(provider, client, provider_config, &message_request).await?;
        let assistant_blocks = response.content;

        let message_text = extract_text(&assistant_blocks);
        if !message_text.is_empty() {
            println!("{message_text}");
        }

        let tool_results = if supports_tools {
            run_tool_calls(workspace_root, &assistant_blocks)
        } else {
            Vec::new()
        };
        messages.push(Message::assistant_blocks(assistant_blocks));
        if !supports_tools {
            finished = true;
            break;
        }
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
    Ok(())
}

pub(crate) fn extract_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
