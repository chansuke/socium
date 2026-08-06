use crate::AgentProgress;
use crate::agent::types::{AgentMessageRequest, ContentBlock, Message};
use crate::constants::MAX_TOOL_ROUNDS;
use crate::providers;
use crate::providers::{DeltaEvent, ProviderConfig, ProviderKind};
use crate::tools::ToolDefinition;
use crate::tools::run_tool_calls_with_progress;
use std::io::Error;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_agent_turn<F>(
    provider: ProviderKind,
    client: &reqwest::Client,
    provider_config: &ProviderConfig,
    model: &str,
    max_tokens: u32,
    supports_tools: bool,
    tools: &[ToolDefinition],
    workspace_root: &Path,
    messages: &mut Vec<Message>,
    on_progress: &mut F,
) -> Result<Vec<String>, Box<dyn std::error::Error>>
where
    F: FnMut(AgentProgress) + Send,
{
    let mut finished = false;
    let mut assistant_text_chunks = Vec::new();
    let max_rounds = if supports_tools { MAX_TOOL_ROUNDS } else { 1 };
    for round in 0..max_rounds {
        on_progress(AgentProgress::ModelRequestStarted {
            round: round + 1,
            provider: provider.to_string(),
        });
        let message_request = AgentMessageRequest::new(model, max_tokens, messages, tools);
        let mut on_delta = |event: DeltaEvent| match event {
            DeltaEvent::Text(text) => on_progress(AgentProgress::AssistantText { text }),
            DeltaEvent::Thinking(text) => on_progress(AgentProgress::ThinkingText { text }),
        };
        let response = providers::send_message(
            provider,
            client,
            provider_config,
            &message_request,
            &mut on_delta,
        )
        .await?;
        let assistant_blocks = response.message.content;

        let message_text = extract_text(&assistant_blocks);
        if !response.streamed_text && !message_text.is_empty() {
            on_progress(AgentProgress::AssistantText {
                text: message_text.clone(),
            });
            assistant_text_chunks.push(message_text);
        } else if !message_text.is_empty() {
            assistant_text_chunks.push(message_text);
        }

        let tool_results = if supports_tools {
            run_tool_calls_with_progress(workspace_root, &assistant_blocks, on_progress)
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
    Ok(assistant_text_chunks)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::ContentBlock;
    use serde_json::json;

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
}
