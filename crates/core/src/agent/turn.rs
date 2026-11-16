use crate::AgentProgress;
use crate::activity::{
    ActivityClock, ActivityKind, RoundActivity, summarize_tool_input, summarize_tool_result,
};
use crate::agent::subagent::catalog::SubagentCatalog;
use crate::agent::subagent::run_subagent;
use crate::agent::types::{AgentMessageRequest, ContentBlock, Message};
use crate::constants::{AGENT_TOOL_NAME, MAX_TOOL_ROUNDS};
use crate::error::AgentError;
use crate::providers::{Provider, ProviderEvent, TokenUsage};
use crate::tools::ToolDefinition;
use crate::tools::error::ToolError;
use crate::tools::run_tool_call_on_blocking_thread;
use serde_json::Value;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::task::Poll;
use std::time::Instant;

#[derive(Clone, Copy)]
pub(crate) struct TurnContext<'a> {
    pub(crate) provider: &'a dyn Provider,
    pub(crate) client: &'a reqwest::Client,
    pub(crate) model: &'a str,
    pub(crate) system: &'a str,
    /// The project's framed instruction file, appended to subagent
    /// prompts; empty when there is none.
    pub(crate) project_instructions: &'a str,
    pub(crate) context_window: Option<u32>,
    pub(crate) max_tokens: u32,
    /// See [`crate::agent::context::ContextBudget::estimate_scale`].
    pub(crate) estimate_scale: f32,
    /// The tools offered to the model; a call to any other is refused.
    pub(crate) tools: &'a [ToolDefinition],
    /// The types the `agent` tool can start.
    pub(crate) subagents: &'a SubagentCatalog,
    pub(crate) control: &'a crate::TaskControl,
    pub(crate) settings: &'a crate::settings::Settings,
    pub(crate) workspace_root: &'a Path,
    /// The main agent's turn, which a subagent's edits belong to as well.
    pub(crate) edit_origin: Option<&'a crate::tools::edit_history::EditOrigin>,
    pub(crate) clock: ActivityClock,
}

pub(crate) struct TurnOutcome {
    pub(crate) assistant_text_chunks: Vec<String>,
    /// Usage measured on the last model call of the turn; approximates the
    /// prompt size the next turn will start from.
    pub(crate) last_usage: Option<TokenUsage>,
    /// Things the user should know about how the turn ended, such as an
    /// answer cut at the output limit.
    pub(crate) notices: Vec<String>,
    /// Measured input per estimated token on the last call, when measured.
    pub(crate) estimate_scale: Option<f32>,
}

/// Further attempts after a retryable failure before any output arrived.
const MAX_RETRIES: u32 = 3;
const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(60);

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
    let mut notices = Vec::new();
    let mut estimate_scale = None;
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
            crate::agent::context::resolve_context_budget(ctx.context_window, ctx.max_tokens)
                .with_scale(estimate_scale.unwrap_or(ctx.estimate_scale));
        let limit = budget.input_budget_tokens();
        let mut estimate = budget.estimate(ctx.model, messages, ctx.tools);
        if estimate > limit
            && let Some(Message {
                content: crate::agent::types::MessageContent::Blocks(results),
                role: crate::agent::types::Role::User,
                ..
            }) = messages.last_mut()
        {
            // The newest results overflowed. Nothing was produced against
            // them yet, so cutting them leaves every thinking block valid;
            // the history keeps what the model is shown.
            let excess = crate::agent::context::excess_characters(
                ctx.model,
                estimate - limit,
                budget.estimate_scale,
            );
            if crate::agent::context::shrink_tool_results(results, excess) > 0 {
                emit(
                    on_progress,
                    ActivityKind::Warning {
                        message: "Tool output was cut to fit the context window".into(),
                    },
                );
                estimate = budget.estimate(ctx.model, messages, ctx.tools);
            }
        }
        if estimate > limit {
            return Err(AgentError::ContextFull(format!(
                "The context window filled during the tool loop (about {estimate} of {limit} tokens)."
            )));
        }
        let estimated_input = crate::agent::context::estimate_request_tokens(
            ctx.model,
            ctx.max_tokens,
            messages,
            ctx.tools,
        );
        // Thinking another provider or model wrote stays in the history
        // but is not sent where it cannot be read.
        let readable = crate::agent::types::readable_thinking(messages, &|origin| {
            ctx.provider.reads_thinking_from(origin)
        });
        let message_request = AgentMessageRequest::with_system_prompt(
            ctx.model,
            ctx.max_tokens,
            ctx.system,
            readable.as_deref().unwrap_or(messages),
            ctx.tools,
        );
        let mut attempt = 0;
        let result = loop {
            let mut round_activity = RoundActivity::start();
            let mut received = false;
            let mut on_delta = |event: ProviderEvent| {
                match &event {
                    ProviderEvent::TextDelta { text, .. } => {
                        received = true;
                        on_progress(AgentProgress::AssistantText { text: text.clone() })
                    }
                    ProviderEvent::ThinkingDelta { text, .. } => {
                        received = true;
                        on_progress(AgentProgress::ThinkingText { text: text.clone() })
                    }
                    ProviderEvent::Raw { json } => on_progress(AgentProgress::RawProviderEvent {
                        turn_id: clock.turn_id(),
                        round: round_number,
                        json: json.clone(),
                    }),
                    ProviderEvent::BlockStarted { .. } | ProviderEvent::ToolInputDelta { .. } => {
                        received = true
                    }
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
            // Retry only while nothing was shown, so the user never sees a
            // reply restart; the request has no side effects of its own.
            if let Err(AgentError::Provider(err)) = &result
                && err.is_retryable()
                && !received
                && attempt < MAX_RETRIES
            {
                attempt += 1;
                let delay = err
                    .retry_after()
                    .unwrap_or(std::time::Duration::from_secs(1 << attempt))
                    .min(MAX_RETRY_DELAY);
                emit(
                    on_progress,
                    ActivityKind::Warning {
                        message: format!(
                            "{err}; retrying in {}s (attempt {} of {})",
                            delay.as_secs(),
                            attempt + 1,
                            MAX_RETRIES + 1
                        ),
                    },
                );
                tokio::select! {
                    biased;
                    _ = ctx.control.cancelled() => {}
                    _ = tokio::time::sleep(delay) => {}
                }
                if ctx.control.is_cancelled() {
                    break Err(AgentError::Cancelled);
                }
                continue;
            }
            // A response that ended without message_stop (an error, a
            // cancellation, or a provider that does not stream) closes here.
            for kind in round_activity.finish(None, result.as_ref().ok().and_then(|r| r.usage)) {
                emit(on_progress, kind);
            }
            break result;
        };
        let response = result?;
        if let Some(usage) = response.usage {
            last_usage = Some(usage);
            estimate_scale =
                crate::agent::context::estimate_scale(usage.total_input_tokens(), estimated_input)
                    .or(estimate_scale);
        }
        let assistant_blocks = response.message.content;
        let invalid_inputs = response.message.invalid_tool_inputs;
        if response.message.stop_reason.as_deref() == Some("max_tokens") {
            let notice = format!(
                "The answer stopped at the output limit of {} tokens; raise MAX_TOKENS or the model's max_output_tokens, or ask it to continue.",
                ctx.max_tokens
            );
            emit(
                on_progress,
                ActivityKind::Warning {
                    message: notice.clone(),
                },
            );
            notices.push(notice);
        }

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
        messages.push(Message::assistant_from(
            assistant_blocks,
            crate::agent::types::Origin {
                provider: ctx.provider.label(),
                model: ctx.model.to_string(),
            },
        ));
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
        let mut next = 0;
        while next < calls.len() {
            let batch = concurrent_batch_len(ctx, &calls[next..]);
            if batch > 1 && !ctx.control.is_cancelled() {
                let batch_calls = &calls[next..next + batch];
                next += batch;
                for (id, name, input) in batch_calls {
                    emit(
                        on_progress,
                        ActivityKind::ToolStarted {
                            call_id: id.clone(),
                            name: name.clone(),
                            input_summary: summarize_tool_input(name, input),
                        },
                    );
                    on_progress(AgentProgress::ToolCallStarted { name: name.clone() });
                }
                // Every intent is saved before any of them starts.
                let outcomes = batch_calls
                    .iter()
                    .map(|(id, name, _)| {
                        checkpoint(
                            messages,
                            serde_json::json!({"type":"tool_started","round":round+1,"attempt":1,"tool_call_id":id,"tool":name,"authorization":"subagent-tools-only","concurrent":true}),
                        )
                        .err()
                        .map(|e| {
                            Err(ToolError::Message(format!(
                                "Could not persist execution intent; tool was not started: {e}"
                            )))
                        })
                    })
                    .collect::<Vec<_>>();
                let shared = std::sync::Mutex::new(&mut *on_progress);
                let shared = &shared;
                let mut runs = batch_calls
                    .iter()
                    .zip(&outcomes)
                    .enumerate()
                    .filter(|(_, (_, outcome))| outcome.is_none())
                    .map(|(index, ((id, name, input), _))| {
                        let run: Pin<Box<dyn Future<Output = _> + Send + '_>> =
                            Box::pin(async move {
                                let started = Instant::now();
                                let mut forward = |event: AgentProgress| {
                                    let mut on_progress =
                                        shared.lock().unwrap_or_else(|e| e.into_inner());
                                    (*on_progress)(event);
                                };
                                let report = run_subagent(ctx, id, input, &mut forward).await;
                                // Shown as each one ends, not when the batch does.
                                let (is_error, content) = match &report {
                                    Ok(content) => (false, content.clone()),
                                    Err(err) => (true, err.to_string()),
                                };
                                forward(AgentProgress::ToolCallFinished {
                                    name: name.clone(),
                                    is_error,
                                });
                                forward(AgentProgress::Activity(clock.event(
                                    round_number,
                                    ActivityKind::ToolFinished {
                                        call_id: id.clone(),
                                        name: name.clone(),
                                        is_error,
                                        elapsed_ms: elapsed_ms(started),
                                        summary: summarize_tool_result(&content),
                                    },
                                )));
                                (index, report)
                            });
                        run
                    })
                    .collect::<Vec<_>>();
                // Each result is saved as its call ends, so a crash keeps the
                // reports already in. It goes where call order puts it; a
                // restart fills the gaps of calls still running.
                let first = results.len();
                let mut placed = Vec::new();
                let mut saved = Ok(());
                let mut not_started = outcomes
                    .into_iter()
                    .enumerate()
                    .filter_map(|(index, outcome)| Some((index, outcome?)));
                loop {
                    let (index, outcome) = match not_started.next() {
                        Some(call) => call,
                        None if runs.is_empty() => break,
                        None => next_finished(&mut runs).await,
                    };
                    let (id, name, _) = &batch_calls[index];
                    let result = match outcome {
                        Ok(content) => ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content,
                            is_error: None,
                        },
                        Err(err) => tool_error(id.clone(), err),
                    };
                    let failed = matches!(
                        result,
                        ContentBlock::ToolResult {
                            is_error: Some(true),
                            ..
                        }
                    );
                    let slot = placed.partition_point(|&earlier| earlier < index);
                    placed.insert(slot, index);
                    results.insert(first + slot, bounded(result));
                    *messages.last_mut().unwrap() = Message::user_blocks(results.clone());
                    // The other children still run to the end before a
                    // failed save ends the turn.
                    saved = saved.and(checkpoint(messages,serde_json::json!({"type":"tool_finished","round":round+1,"attempt":1,"tool_call_id":id,"tool":name,"is_error":failed,"result_index":first+index})));
                }
                drop(runs);
                // The conversation lists the calls in call order, as a
                // restart rebuilds it.
                for ((_, name, input), result) in batch_calls.iter().zip(&results[first..]) {
                    if let ContentBlock::ToolResult {
                        content, is_error, ..
                    } = result
                    {
                        on_progress(AgentProgress::ToolCallRecorded(
                            crate::transcript::tool_entry(
                                name,
                                input,
                                Some((content, *is_error == Some(true))),
                                false,
                            ),
                        ));
                    }
                }
                saved.map_err(|e|std::io::Error::other(format!("Tool finished but its result could not be saved; outcome may be unknown after restart: {e}")))?;
                continue;
            }
            let (id, name, input) = calls[next].clone();
            next += 1;
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
            let mut approved = false;
            let mut tool_progress = |event: AgentProgress| {
                let kind = match &event {
                    AgentProgress::ApprovalRequested { tool, .. } => {
                        approval_started = Some(Instant::now());
                        Some(ActivityKind::ApprovalWaiting {
                            call_id: call_id.clone(),
                            tool: tool.clone(),
                        })
                    }
                    AgentProgress::ApprovalResolved {
                        approved: allowed, ..
                    } => {
                        approved |= *allowed;
                        Some(ActivityKind::ApprovalResolved {
                            call_id: call_id.clone(),
                            approved: *allowed,
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
            let offered = ctx.tools.iter().any(|tool| tool.name() == name);
            let invalid = invalid_inputs.iter().find(|(invalid, _)| *invalid == id);
            let result = if ctx.control.is_cancelled() {
                ContentBlock::ToolResult {
                    tool_use_id: id,
                    content: "Cancelled before execution; no side effects".into(),
                    is_error: Some(true),
                }
            } else if let Some((_, error)) = invalid {
                // Never run a call whose arguments did not parse.
                tool_error(
                    id,
                    ToolError::Message(format!(
                        "The arguments of this {name} call were not valid JSON ({error}); nothing ran. Send the call again with complete arguments."
                    )),
                )
            } else if !offered {
                // A model may name a tool it was not given, such as a
                // subagent asking for one outside its type.
                tool_error(id, ToolError::UnsupportedTool(name.clone()))
            } else if name == AGENT_TOOL_NAME {
                tool_progress(AgentProgress::ToolCallStarted { name: name.clone() });
                let started = checkpoint(
                    messages,
                    serde_json::json!({"type":"tool_started","round":round+1,"attempt":1,"tool_call_id":id,"tool":name,"authorization":"subagent-tools-only"}),
                );
                let report = match started {
                    Err(e) => Err(ToolError::Message(format!(
                        "Could not persist execution intent; tool was not started: {e}"
                    ))),
                    Ok(()) => run_subagent(ctx, &id, &input, &mut tool_progress).await,
                };
                tool_progress(AgentProgress::ToolCallFinished {
                    name: name.clone(),
                    is_error: report.is_err(),
                });
                match report {
                    Ok(content) => ContentBlock::ToolResult {
                        tool_use_id: id,
                        content,
                        is_error: None,
                    },
                    Err(err) => tool_error(id, err),
                }
            } else {
                run_tool_call_on_blocking_thread(
                    ctx.workspace_root,
                    &id,
                    &name,
                    &input,
                    ctx.control,
                    ctx.settings,
                    ctx.edit_origin,
                    &mut tool_progress,
                    &mut |authorization| {
                        // The input is already in the message; the event
                        // records only what decided the call.
                        checkpoint(
                            messages,
                            serde_json::json!({"type":"tool_started","round":round+1,"attempt":1,"tool_call_id":id,"tool":name,"authorization":authorization}),
                        )
                    },
                )
                .await
            };
            let failed = matches!(
                result,
                ContentBlock::ToolResult {
                    is_error: Some(true),
                    ..
                }
            );
            if let ContentBlock::ToolResult { content, .. } = &result {
                on_progress(AgentProgress::ToolCallRecorded(
                    crate::transcript::tool_entry(
                        &tool_name,
                        &input,
                        Some((content, failed)),
                        approved,
                    ),
                ));
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
            results.push(bounded(result));
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
        notices,
        estimate_scale,
    })
}

/// How many calls from the start of `calls` can run at once: consecutive
/// new subagents whose types cannot change the workspace, so they never
/// ask for approval or edit the same file. A resumed subagent runs alone.
fn concurrent_batch_len(ctx: &TurnContext<'_>, calls: &[(String, String, Value)]) -> usize {
    calls
        .iter()
        .take_while(|(_, name, input)| {
            name == AGENT_TOOL_NAME
                && ctx.tools.iter().any(|tool| tool.name() == name)
                && input.get("resume").is_none_or(Value::is_null)
                && input
                    .get("agent_type")
                    .and_then(Value::as_str)
                    .and_then(|agent_type| ctx.subagents.find(agent_type))
                    .is_some_and(|kind| !kind.tools.may_change_workspace())
        })
        .count()
}

/// Polls `futures` together on the current task until one finishes, then
/// removes it and returns its output.
async fn next_finished<'a, T>(
    futures: &mut Vec<Pin<Box<dyn Future<Output = T> + Send + 'a>>>,
) -> T {
    std::future::poll_fn(|cx| {
        for index in 0..futures.len() {
            if let Poll::Ready(output) = futures[index].as_mut().poll(cx) {
                drop(futures.swap_remove(index));
                return Poll::Ready(output);
            }
        }
        Poll::Pending
    })
    .await
}

/// One result keeps at most `MAX_TOOL_RESULT_CHARACTERS`, its head and
/// tail, so a single call cannot fill the window.
fn bounded(result: ContentBlock) -> ContentBlock {
    match result {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => ContentBlock::ToolResult {
            tool_use_id,
            content: crate::agent::context::elide_tool_output(
                &content,
                crate::constants::MAX_TOOL_RESULT_CHARACTERS,
                "from this result",
            ),
            is_error,
        },
        other => other,
    }
}

fn tool_error(tool_use_id: String, err: ToolError) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id,
        content: err.to_string(),
        is_error: Some(true),
    }
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
pub(crate) fn repair_incomplete_tools(messages: &mut Vec<Message>) -> bool {
    use crate::agent::types::{MessageContent, Role};
    let mut repaired = false;
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
                repaired = true;
            }
            // Results go in call order; a concurrent batch saves them as its
            // calls end, leaving gaps for the calls still running.
            if let MessageContent::Blocks(results) = &mut messages[i + 1].content {
                for (index, id) in ids.into_iter().enumerate() {
                    let saved = results[index..].iter().position(|b| matches!(b,ContentBlock::ToolResult{tool_use_id,..} if *tool_use_id==id));
                    let result = match saved {
                        Some(0) => continue,
                        Some(offset) => results.remove(index + offset),
                        None => ContentBlock::ToolResult {tool_use_id:id, content:"Interrupted before result was saved. Outcome unknown; inspect workspace before retrying. Do not automatically repeat this operation.".into(), is_error:Some(true)},
                    };
                    results.insert(index, result);
                    repaired = true;
                }
            }
            i += 1;
        }
        i += 1;
    }
    repaired
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
    fn only_consecutive_new_read_only_subagents_are_batched() {
        use crate::agent::subagent::catalog::SubagentCatalog;
        use crate::test_support::{ScriptedProvider, make_temp_dir};
        let workspace = make_temp_dir("batch");
        let provider = ScriptedProvider::new(Vec::new());
        let client = reqwest::Client::new();
        let tools = crate::tools::tools_for_model(true);
        let subagents = SubagentCatalog::builtin();
        let control = crate::TaskControl::default();
        let ctx = TurnContext {
            provider: &provider,
            client: &client,
            model: "m",
            system: "s",
            project_instructions: "",
            context_window: None,
            max_tokens: 100,
            estimate_scale: 1.0,
            tools: &tools,
            subagents: &subagents,
            control: &control,
            settings: &crate::settings::Settings::default(),
            workspace_root: workspace.path(),
            edit_origin: None,
            clock: ActivityClock::start(0),
        };
        let agent = |agent_type: &str| {
            (
                "id".to_string(),
                "agent".to_string(),
                json!({"agent_type": agent_type}),
            )
        };
        let resumed = (
            "id".to_string(),
            "agent".to_string(),
            json!({"agent_type": "explorer", "resume": "x"}),
        );
        let read = (
            "id".to_string(),
            "read_file".to_string(),
            json!({"path": "a"}),
        );

        let explorer = agent("explorer");
        assert_eq!(
            concurrent_batch_len(&ctx, &[explorer.clone(), explorer.clone(), agent("worker")]),
            2
        );
        assert_eq!(
            concurrent_batch_len(&ctx, &[agent("worker"), explorer.clone()]),
            0
        );
        assert_eq!(
            concurrent_batch_len(&ctx, &[explorer.clone(), resumed, explorer.clone()]),
            1
        );
        assert_eq!(
            concurrent_batch_len(&ctx, &[explorer.clone(), read, explorer.clone()]),
            1
        );
        assert_eq!(concurrent_batch_len(&ctx, &[agent("unknown"), explorer]), 0);
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
}
