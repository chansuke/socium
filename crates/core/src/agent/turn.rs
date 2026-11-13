use crate::AgentProgress;
use crate::activity::{
    ActivityClock, ActivityKind, RoundActivity, summarize_tool_input, summarize_tool_result,
};
use crate::agent::types::{AgentMessageRequest, ContentBlock, Message};
use crate::constants::MAX_TOOL_ROUNDS;
use crate::error::AgentError;
use crate::providers::{Provider, ProviderEvent, TokenUsage};
use crate::tools::ToolDefinition;
use crate::tools::run_tool_call_checkpointed;
use std::path::Path;
use std::time::Instant;

pub(crate) struct TurnContext<'a> {
    pub(crate) provider: &'a dyn Provider,
    pub(crate) client: &'a reqwest::Client,
    pub(crate) model: &'a str,
    pub(crate) context_window: Option<u32>,
    pub(crate) max_tokens: u32,
    pub(crate) tools: &'a [ToolDefinition],
    pub(crate) control: &'a crate::TaskControl,
    pub(crate) workspace_root: &'a Path,
    pub(crate) clock: ActivityClock,
}

pub(crate) struct TurnOutcome {
    pub(crate) assistant_text_chunks: Vec<String>,
    /// Usage measured on the last model call of the turn; approximates the
    /// prompt size the next turn will start from.
    pub(crate) last_usage: Option<TokenUsage>,
}

pub(crate) async fn run_agent_turn<F>(
    ctx: &TurnContext<'_>,
    messages: &mut Vec<Message>,
    on_progress: &mut F,
    checkpoint: &mut (impl FnMut(&[Message], serde_json::Value) -> std::io::Result<()> + Send),
) -> Result<TurnOutcome, AgentError>
where
    F: FnMut(AgentProgress) + Send,
{
    let supports_tools = ctx.provider.capabilities().supports_tools;
    let mut finished = false;
    let mut assistant_text_chunks = Vec::new();
    let mut last_usage = None;
    let max_rounds = if supports_tools { MAX_TOOL_ROUNDS } else { 1 };
    let clock = ctx.clock;
    for round in 0..max_rounds {
        let round_number = round + 1;
        let emit = |on_progress: &mut F, kind: ActivityKind| {
            on_progress(AgentProgress::Activity(clock.event(round_number, kind)));
        };
        on_progress(AgentProgress::ModelRequestStarted {
            round: round_number,
            provider: ctx.provider.label(),
        });
        emit(
            on_progress,
            ActivityKind::RoundStarted {
                model: ctx.model.to_string(),
            },
        );
        validate_protocol(messages)?;
        let budget =
            crate::agent::context::resolve_context_budget(ctx.context_window, ctx.max_tokens)?;
        if crate::agent::context::estimate_request_tokens(
            ctx.model,
            ctx.max_tokens,
            messages,
            ctx.tools,
        ) > budget.input_budget_tokens()
        {
            return Err(std::io::Error::other(
                "Context budget exceeded during tool loop; compact before continuing",
            )
            .into());
        }
        let message_request =
            AgentMessageRequest::new(ctx.model, ctx.max_tokens, messages, ctx.tools);
        let mut round_activity = RoundActivity::start();
        let mut on_delta = |event: ProviderEvent| {
            match &event {
                ProviderEvent::TextDelta { text, .. } => {
                    on_progress(AgentProgress::AssistantText { text: text.clone() })
                }
                ProviderEvent::ThinkingDelta { text, .. } => {
                    on_progress(AgentProgress::ThinkingText { text: text.clone() })
                }
                ProviderEvent::Raw { json } => on_progress(AgentProgress::RawProviderEvent {
                    turn_id: clock.turn_id(),
                    round: round_number,
                    json: json.clone(),
                }),
                _ => {}
            }
            for kind in round_activity.apply(event) {
                on_progress(AgentProgress::Activity(clock.event(round_number, kind)));
            }
        };
        let result = tokio::select! {
            biased;
            _ = ctx.control.cancelled() => Err(AgentError::Cancelled),
            result = ctx.provider.send_message(ctx.client, &message_request, &mut on_delta) => result.map_err(AgentError::from),
        };
        // A response that ended without message_stop (an error, a
        // cancellation, or a provider that does not stream) closes here.
        for kind in round_activity.finish(None, result.as_ref().ok().and_then(|r| r.usage)) {
            emit(on_progress, kind);
        }
        let response = result?;
        if response.usage.is_some() {
            last_usage = response.usage;
        }
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

        let calls: Vec<_> = assistant_blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => {
                    Some((id.clone(), name.clone(), input.clone()))
                }
                _ => None,
            })
            .collect();
        let mut ids = std::collections::HashSet::new();
        if calls
            .iter()
            .any(|(id, _, _)| id.is_empty() || !ids.insert(id.clone()))
        {
            return Err(crate::providers::error::ProviderError::Stream(
                "Duplicate/empty tool call ID".into(),
            )
            .into());
        }
        messages.push(Message::assistant_blocks(assistant_blocks));
        // Persist intent before side effects. On restart unresolved calls become
        // explicitly unknown results and are never automatically re-executed.
        checkpoint(
            messages,
            serde_json::json!({"type":"assistant_intent","round":round+1,"attempt":1}),
        )?;
        if !supports_tools || calls.is_empty() {
            finished = true;
            break;
        }
        let mut results = Vec::new();
        messages.push(Message::user_blocks(Vec::new()));
        for (id, name, input) in calls {
            let call_id = id.clone();
            let tool_name = name.clone();
            let tool_started = Instant::now();
            emit(
                on_progress,
                ActivityKind::ToolStarted {
                    call_id: id.clone(),
                    name: name.clone(),
                    input_summary: summarize_tool_input(&name, &input),
                },
            );
            let mut approval_started = None;
            let mut tool_progress = |event: AgentProgress| {
                let kind = match &event {
                    AgentProgress::ApprovalRequested { tool, .. } => {
                        approval_started = Some(Instant::now());
                        Some(ActivityKind::ApprovalWaiting {
                            call_id: call_id.clone(),
                            tool: tool.clone(),
                        })
                    }
                    AgentProgress::ApprovalResolved { approved, .. } => {
                        Some(ActivityKind::ApprovalResolved {
                            call_id: call_id.clone(),
                            approved: *approved,
                            waited_ms: approval_started.take().map_or(0, elapsed_ms),
                        })
                    }
                    AgentProgress::CommandOutput {
                        stdout_bytes,
                        stderr_bytes,
                    } => Some(ActivityKind::CommandOutput {
                        call_id: call_id.clone(),
                        stdout_bytes: *stdout_bytes,
                        stderr_bytes: *stderr_bytes,
                    }),
                    _ => None,
                };
                on_progress(event);
                if let Some(kind) = kind {
                    on_progress(AgentProgress::Activity(clock.event(round_number, kind)));
                }
            };
            let result = if ctx.control.is_cancelled() {
                ContentBlock::ToolResult {
                    tool_use_id: id,
                    content: "Cancelled before execution; no side effects".into(),
                    is_error: Some(true),
                }
            } else {
                run_tool_call_checkpointed(
                    ctx.workspace_root,
                    &id,
                    &name,
                    &input,
                    ctx.control,
                    &mut tool_progress,
                    &mut || {
                        checkpoint(
                            messages,
                            serde_json::json!({"type":"tool_started","round":round+1,"attempt":1,"tool_call_id":id,"tool":name,"input":input,"authorization":if matches!(name.as_str(),"run_command"|"git_commit"){"human-approved-once"}else{"workspace-file-policy"}}),
                        )
                    },
                )
            };
            let failed = matches!(
                result,
                ContentBlock::ToolResult {
                    is_error: Some(true),
                    ..
                }
            );
            if let ContentBlock::ToolResult { content, .. } = &result {
                emit(
                    on_progress,
                    ActivityKind::ToolFinished {
                        call_id: call_id.clone(),
                        name: tool_name.clone(),
                        is_error: failed,
                        elapsed_ms: elapsed_ms(tool_started),
                        summary: summarize_tool_result(content),
                    },
                );
            }
            results.push(result);
            *messages.last_mut().unwrap() = Message::user_blocks(results.clone());
            checkpoint(messages,serde_json::json!({"type":"tool_finished","round":round+1,"attempt":1,"tool_call_id":call_id,"tool":tool_name,"is_error":failed,"result_index":results.len()-1})).map_err(|e|std::io::Error::other(format!("Tool finished but its result could not be saved; outcome may be unknown after restart: {e}")))?;
        }
        if ctx.control.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
    }

    if !finished {
        return Err(AgentError::ToolRoundsExhausted);
    }
    Ok(TurnOutcome {
        assistant_text_chunks,
        last_usage,
    })
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn validate_protocol(messages: &[Message]) -> Result<(), AgentError> {
    use crate::agent::types::{MessageContent, Role};
    let invalid = || {
        std::io::Error::other(
            "Invalid conversation tool-call/result protocol; original history preserved",
        )
    };
    let mut pending = std::collections::HashSet::new();
    for message in messages {
        if matches!(message.role, Role::System) {
            return Err(invalid().into());
        }
        let mut calls = std::collections::HashSet::new();
        let mut results = std::collections::HashSet::new();
        if let MessageContent::Blocks(blocks) = &message.content {
            for block in blocks {
                match block {
                    ContentBlock::ToolUse { id, .. } => {
                        if !matches!(message.role, Role::Assistant) || !calls.insert(id.clone()) {
                            return Err(invalid().into());
                        }
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        if !matches!(message.role, Role::User)
                            || !results.insert(tool_use_id.clone())
                        {
                            return Err(invalid().into());
                        }
                    }
                    _ => {}
                }
            }
        }
        if pending != results {
            return Err(invalid().into());
        }
        pending = calls;
    }
    if !pending.is_empty() {
        return Err(invalid().into());
    }
    Ok(())
}

/// Repair a crash between intent and result persistence without guessing whether
/// a side effect happened. This also completes partially recorded tool batches.
pub(crate) fn repair_incomplete_tools(messages: &mut Vec<Message>) {
    use crate::agent::types::{MessageContent, Role};
    let mut i = 0;
    while i < messages.len() {
        let ids: Vec<String> = match &messages[i].content {
            MessageContent::Blocks(blocks) if matches!(messages[i].role, Role::Assistant) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        if !ids.is_empty() {
            let has_results = messages.get(i+1).is_some_and(|m| matches!(&m.content,MessageContent::Blocks(b) if matches!(m.role,Role::User) && b.iter().all(|b| matches!(b,ContentBlock::ToolResult{..}))));
            if !has_results {
                messages.insert(i + 1, Message::user_blocks(Vec::new()));
            }
            if let MessageContent::Blocks(results) = &mut messages[i + 1].content {
                for id in ids {
                    if !results.iter().any(|b| matches!(b,ContentBlock::ToolResult{tool_use_id,..} if *tool_use_id==id)) {
                        results.push(ContentBlock::ToolResult {tool_use_id:id, content:"Interrupted before result was saved. Outcome unknown; inspect workspace before retrying. Do not automatically repeat this operation.".into(), is_error:Some(true)});
                    }
                }
            }
            i += 1;
        }
        i += 1;
    }
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
