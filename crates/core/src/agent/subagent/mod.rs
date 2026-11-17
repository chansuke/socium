//! The `agent` tool: runs a delegated task in a fresh context with its own
//! tool loop and returns only the subagent's final report as the tool result.
//!
//! A subagent shares the main agent's workspace, cancellation, and
//! approvals, and its model unless its type names another. It never sees
//! the main conversation, and its tools are a subset chosen by its type that
//! never includes `agent`, so subagents do not nest.

pub(crate) mod catalog;
pub(crate) mod record;

use crate::AgentProgress;
use crate::activity::{ActivityEvent, ActivityKind, summarize_tool_input};
use crate::agent::compaction::compact_history;
use crate::agent::context::{
    history_input_tokens, prepare_context_messages, resolve_context_budget,
};
use crate::agent::subagent::record::{SubagentRecord, SubagentStatus};
use crate::agent::turn::repair_incomplete_tools;
use crate::agent::turn::{TurnContext, extract_text, run_agent_turn};
use crate::agent::types::{Message, Role};
use crate::constants::{
    AGENT_TOOL_NAME, MAX_SUBAGENT_REPORT_CHARACTERS, MAX_TOOL_ROUNDS, SUBAGENT_FRAMING,
};
use crate::error::AgentError;
use crate::providers::TokenUsage;
use crate::tools::error::ToolError;
use crate::tools::input::{optional_string_field, require_object_input, require_string_field};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

type ReportFuture<'a> = Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>>;

/// Runs the subagent of one `agent` call to the end and returns its report.
///
/// Its activity is forwarded tagged with `call_id`; its streamed text is
/// not, since it reaches the main agent only as the report. Its messages
/// are saved as a record named in the report, which `resume` continues.
///
/// Boxed because it runs a tool loop from inside one.
pub(crate) fn run_subagent<'a>(
    ctx: &'a TurnContext<'_>,
    call_id: &'a str,
    input: &'a Value,
    on_progress: &'a mut (dyn FnMut(AgentProgress) + Send),
) -> ReportFuture<'a> {
    Box::pin(async move {
        let fields = require_object_input(input, AGENT_TOOL_NAME)?;
        let description = require_string_field(fields, "description", AGENT_TOOL_NAME)?;
        let prompt = require_string_field(fields, "prompt", AGENT_TOOL_NAME)?;
        let agent_type = require_string_field(fields, "agent_type", AGENT_TOOL_NAME)?;
        let kind = ctx.subagents.find(agent_type).ok_or_else(|| {
            ToolError::Message(format!(
                "Unknown agent_type '{agent_type}'; use one of: {}",
                ctx.subagents.names().join(", ")
            ))
        })?;
        for (field, value) in [("prompt", prompt), ("description", description)] {
            if value.trim().is_empty() {
                return Err(ToolError::Message(format!(
                    "agent requires a non-empty {field}"
                )));
            }
        }
        // The call's model, else the type's, else the current one.
        let alias =
            optional_string_field(fields, "model", AGENT_TOOL_NAME)?.or(kind.model.as_deref());
        let model = match alias {
            None => None,
            Some(alias) => Some(ctx.subagents.model(alias).ok_or_else(|| {
                let aliases = ctx.subagents.model_aliases();
                ToolError::Message(if aliases.is_empty() {
                    format!(
                        "Model '{alias}' is not available to subagents, and no other model is; leave model out to run on the current one"
                    )
                } else {
                    format!(
                        "Model '{alias}' is not available to subagents; use one of: {}",
                        aliases.join(", ")
                    )
                })
            })?),
        };

        // The type's tools, as its model is offered them.
        let model_tools;
        let child = match model {
            None => *ctx,
            Some(model) => {
                model_tools = crate::tools::tools_with_settings(ctx.settings);
                // Its compaction model when that one can run, else itself,
                // as for the main agent.
                let summarizer = model
                    .config
                    .compaction_alias()
                    .and_then(|alias| ctx.subagents.model(alias))
                    .unwrap_or(model);
                TurnContext {
                    provider: model.provider.as_ref(),
                    model: &model.config.id,
                    context_window: model.config.context_window,
                    max_tokens: model.max_tokens,
                    estimate_scale: 1.0,
                    compaction: (summarizer.provider.as_ref(), &summarizer.config.id),
                    tools: &model_tools,
                    ..*ctx
                }
            }
        };
        let tools = child
            .tools
            .iter()
            .filter(|tool| kind.tools.allows(tool.name()))
            .cloned()
            .collect::<Vec<_>>();
        let system = format!(
            "{SUBAGENT_FRAMING}\n\n{}{}",
            kind.instructions, ctx.project_instructions
        );
        let child = TurnContext {
            system: &system,
            tools: &tools,
            ..child
        };
        let (root, session) = (ctx.workspace_root, ctx.session_id);
        let state_error =
            |err: std::io::Error| ToolError::Message(format!("Subagent record: {err}"));
        let mut record = match optional_string_field(fields, "resume", AGENT_TOOL_NAME)? {
            Some(id) => {
                let mut record = record::load(root, session, id)
                    .map_err(state_error)?
                    .ok_or_else(|| {
                        ToolError::Message(format!(
                            "No saved subagent '{id}' in this session; only subagents started in this session can be resumed"
                        ))
                    })?;
                if record.agent_type != kind.name {
                    return Err(ToolError::Message(format!(
                        "Subagent '{id}' has agent_type '{}'; resume it with that type",
                        record.agent_type
                    )));
                }
                // A crash may have cut its last tool batch short. Thinking
                // from another model stays in the record; the turn sends
                // only what the current model can read.
                repair_incomplete_tools(&mut record.messages);
                record
            }
            None => {
                let id = record::new_id(root, session, call_id).map_err(state_error)?;
                SubagentRecord::new(id, &kind.name, description, child.model)
            }
        };
        let resumed_from = record.messages.len();
        record.messages.push(Message::user_text(prompt.to_string()));
        record.model = child.model.to_string();
        record.runs += 1;
        record.status = SubagentStatus::Running;
        record.save(root, session).map_err(|err| {
            ToolError::Message(format!(
                "Could not save the subagent record; it was not started: {err}"
            ))
        })?;
        if record.runs == 1 {
            // Pruning only removes other, older records.
            let _ = record::prune(root, session, &record.id);
        }
        let agent = Some(call_id.to_string());
        let requested_by = format!(
            "{} subagent \"{}\"",
            kind.name,
            summarize_tool_input(AGENT_TOOL_NAME, input)
        );
        let mut stats = RunStats::start();
        let mut forward = |event: AgentProgress| match event {
            AgentProgress::Activity(mut activity) => {
                stats.observe(&activity);
                activity.agent.clone_from(&agent);
                on_progress(AgentProgress::Activity(activity));
            }
            AgentProgress::ApprovalRequested {
                request_id,
                tool,
                summary,
                input,
                requested_by: _,
                session_option,
            } => on_progress(AgentProgress::ApprovalRequested {
                request_id,
                tool,
                summary,
                input,
                requested_by: Some(requested_by.clone()),
                session_option,
            }),
            // These would read as the main agent's own stream or calls; the
            // child's calls are in the Activity pane.
            AgentProgress::AssistantText { .. }
            | AgentProgress::ThinkingText { .. }
            | AgentProgress::RawProviderEvent { .. }
            | AgentProgress::ModelRequestStarted { .. }
            | AgentProgress::ToolCallRecorded(_) => {}
            // Approvals, the status line and command output.
            other @ (AgentProgress::ApprovalRequired { .. }
            | AgentProgress::ApprovalResolved { .. }
            | AgentProgress::ToolCallStarted { .. }
            | AgentProgress::ToolCallFinished { .. }
            | AgentProgress::CommandOutput { .. }
            | AgentProgress::CommandStarted { .. }
            | AgentProgress::CommandFinished { .. }
            | AgentProgress::CompactionStarted
            | AgentProgress::CompactionFinished { .. }) => on_progress(other),
        };
        // A call runs once its intent is saved, so the saved intents are
        // the calls that ran; one refused or denied before that has none.
        let intents_before = record.tool_events.len();
        let result = run_in_window(&child, &mut record, resumed_from, &mut forward).await;
        stats.tool_calls = record.tool_events.len() - intents_before;
        let answered = record.messages[resumed_from..]
            .iter()
            .any(|message| matches!(message.role, Role::Assistant));
        if resumed_from > 0 && result.is_err() && !answered {
            // A follow-up the model never answered is not kept, so trying
            // it again does not stack prompts.
            record.messages.truncate(resumed_from);
            record.runs -= 1;
        }
        record.status = match &result {
            Ok(_) => SubagentStatus::Finished,
            Err(AgentError::Cancelled) => SubagentStatus::Cancelled,
            Err(_) => SubagentStatus::Failed,
        };
        let saved = record.save(root, session);

        let notes = last_assistant_text(&record.messages[resumed_from..]);
        let side_effects = if kind.tools.may_change_workspace() {
            " Changes it made before stopping are kept; inspect the workspace before retrying."
        } else {
            ""
        };
        let mut stats = stats.describe(&record.id);
        if let Err(err) = saved {
            stats.push_str(&format!("\nThe final record could not be saved: {err}"));
        }
        match result {
            Ok(_) if notes.trim().is_empty() => Ok(format!(
                "The subagent finished without a report.\n\n{stats}"
            )),
            Ok(_) => Ok(format!("{}\n\n{stats}", bounded_report(notes))),
            Err(AgentError::ToolRoundsExhausted) => Err(ToolError::Message(format!(
                "The subagent stopped after {MAX_TOOL_ROUNDS} rounds without finishing.{side_effects}{}\n\n{stats}",
                last_notes(&notes)
            ))),
            Err(AgentError::Cancelled) => Err(ToolError::Message(format!(
                "The subagent was cancelled.{side_effects}\n\n{stats}"
            ))),
            Err(err) => Err(ToolError::Message(format!(
                "The subagent failed: {err}.{side_effects}{}\n\n{stats}",
                last_notes(&notes)
            ))),
        }
    })
}

/// Runs the subagent's tool loop on what of `record` fits its window,
/// saving the record at every checkpoint. A history that does not fit is
/// summarized first, and a loop whose window fills goes on from the
/// summary, as the main agent does when the user asks it to continue.
async fn run_in_window(
    ctx: &TurnContext<'_>,
    record: &mut SubagentRecord,
    run_from: usize,
    on_progress: &mut (impl FnMut(AgentProgress) + Send),
) -> Result<(), AgentError> {
    let budget =
        resolve_context_budget(ctx.context_window, ctx.max_tokens).with_scale(ctx.estimate_scale);
    let rounds = |record: &SubagentRecord| {
        record.messages[run_from..]
            .iter()
            .filter(|message| matches!(message.role, Role::Assistant))
            .count()
    };
    loop {
        let history = history_input_tokens(
            &record.messages,
            &record.summary_state,
            ctx.model,
            budget,
            ctx.tools,
        );
        if history > budget.input_budget_tokens() {
            compact_record(ctx, record, on_progress).await?;
        }
        let prepared = prepare_context_messages(
            &record.messages,
            &record.summary_state,
            budget,
            ctx.model,
            ctx.tools,
        )?;
        record.summary_state = prepared.summary_state;
        let mut context = prepared.messages;
        let (raw_len, initial_len) = (record.messages.len(), context.len());
        let rounds_done = rounds(record);
        let segment = TurnContext {
            rounds_done,
            ..*ctx
        };
        // Saved before each side effect, like the main conversation.
        let mut checkpoint = |context: &[Message], event: Value| -> std::io::Result<()> {
            record.messages.truncate(raw_len);
            record.messages.extend_from_slice(&context[initial_len..]);
            let started = event["type"] == "tool_started";
            if started {
                record.tool_events.push(event);
            }
            let saved = record.save(ctx.workspace_root, ctx.session_id);
            if saved.is_err() && started {
                record.tool_events.pop();
            }
            saved
        };
        match run_agent_turn(&segment, &mut context, None, on_progress, &mut checkpoint).await {
            // A loop that got no answer would fill the window again.
            Err(AgentError::ContextFull(_)) if rounds(record) > rounds_done => {}
            result => return result.map(drop),
        }
    }
}

/// Summarizes the older part of the record, reporting it as the
/// subagent's activity. With nothing to summarize, preparing the context
/// still trims what is sent.
async fn compact_record(
    ctx: &TurnContext<'_>,
    record: &mut SubagentRecord,
    on_progress: &mut (impl FnMut(AgentProgress) + Send),
) -> Result<(), AgentError> {
    let mut emit =
        |kind: ActivityKind| on_progress(AgentProgress::Activity(ctx.clock.event(0, kind)));
    emit(ActivityKind::CompactionStarted);
    let compaction = match compact_history(
        ctx.compaction,
        ctx.client,
        ctx.control,
        ctx.model,
        &record.messages,
        &record.summary_state,
    )
    .await
    {
        Err(AgentError::NothingToCompact) => return Ok(()),
        compaction => compaction?,
    };
    if let Some(message) = compaction.fallback_reason.clone() {
        emit(ActivityKind::Warning { message });
    }
    emit(ActivityKind::CompactionFinished {
        covered_messages: compaction.covered_messages,
        used_llm: compaction.fallback_reason.is_none(),
    });
    record.summary_state = compaction.summary_state;
    Ok(())
}

/// What one subagent run cost: the model requests it made and the tool
/// calls it ran, measured also for failed and cancelled runs.
struct RunStats {
    started: Instant,
    rounds: usize,
    tool_calls: usize,
    usage: Option<TokenUsage>,
}

impl RunStats {
    fn start() -> Self {
        Self {
            started: Instant::now(),
            rounds: 0,
            tool_calls: 0,
            usage: None,
        }
    }

    fn observe(&mut self, event: &ActivityEvent) {
        // A round ends only after its request, so one that failed before
        // sending is left out.
        let ActivityKind::RoundFinished { usage, .. } = &event.kind else {
            return;
        };
        self.rounds += 1;
        if let Some(usage) = usage {
            let total = self.usage.get_or_insert_with(TokenUsage::default);
            total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
            total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
            total.cache_creation_input_tokens = total
                .cache_creation_input_tokens
                .saturating_add(usage.cache_creation_input_tokens);
            total.cache_read_input_tokens = total
                .cache_read_input_tokens
                .saturating_add(usage.cache_read_input_tokens);
        }
    }

    /// One line for the end of the tool result, such as `(subagent
    /// toolu_1: 2 rounds, 1 tool call, 1200 input + 300 output tokens,
    /// 4.1s)`.
    fn describe(&self, id: &str) -> String {
        let plural = |count: usize, word: &str| {
            format!("{count} {word}{}", if count == 1 { "" } else { "s" })
        };
        let tokens = match self.usage {
            Some(usage) => format!(
                "{} input + {} output tokens",
                usage.total_input_tokens(),
                usage.output_tokens
            ),
            None => "token usage not reported".to_string(),
        };
        format!(
            "(subagent {id}: {}, {}, {tokens}, {:.1}s)",
            plural(self.rounds, "round"),
            plural(self.tool_calls, "tool call"),
            self.started.elapsed().as_secs_f64()
        )
    }
}

/// Text of the newest assistant message, which on success is the report.
fn last_assistant_text(messages: &[Message]) -> String {
    use crate::agent::types::{MessageContent, Role};
    messages
        .iter()
        .rev()
        .find_map(|message| match (&message.role, &message.content) {
            (Role::Assistant, MessageContent::Blocks(blocks)) => Some(extract_text(blocks)),
            _ => None,
        })
        .unwrap_or_default()
}

fn last_notes(notes: &str) -> String {
    if notes.trim().is_empty() {
        String::new()
    } else {
        format!("\nIts last notes:\n{}", bounded_report(notes.to_string()))
    }
}

fn bounded_report(mut report: String) -> String {
    if let Some((cut, _)) = report.char_indices().nth(MAX_SUBAGENT_REPORT_CHARACTERS) {
        report.truncate(cut);
        report.push_str(&format!(
            "\n… report cut at {MAX_SUBAGENT_REPORT_CHARACTERS} characters"
        ));
    }
    report
}

#[cfg(test)]
mod tests {
    use crate::agent::subagent::catalog::MAIN_AGENT_TOOLS;
    use crate::agent::types::{AgentMessageResponse, ContentBlock};
    use crate::constants::{
        DEFAULT_COMPACTION_MODEL, DEFAULT_SYSTEM_PROMPT, SUBAGENT_EXPLORER_INSTRUCTIONS,
        SUBAGENT_FRAMING, SUBAGENT_WORKER_INSTRUCTIONS,
    };
    use crate::providers::error::ProviderError;
    use crate::providers::{ProviderResponse, TokenUsage};
    use crate::test_support::{ScriptedProvider, make_temp_dir, text_provider_response};
    use crate::{ActivityKind, AgentProgress, AgentService, TranscriptEntry, TranscriptRole};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    fn calls(calls: &[(&str, &str, Value)]) -> Result<ProviderResponse, ProviderError> {
        Ok(ProviderResponse {
            message: AgentMessageResponse {
                content: calls
                    .iter()
                    .map(|(id, name, input)| ContentBlock::ToolUse {
                        id: (*id).into(),
                        name: (*name).into(),
                        input: input.clone(),
                    })
                    .collect(),
                ..Default::default()
            },
            streamed_text: false,
            usage: None,
        })
    }

    fn text(text: &str) -> Result<ProviderResponse, ProviderError> {
        Ok(text_provider_response(text, None))
    }

    fn delegate(agent_type: &str) -> Result<ProviderResponse, ProviderError> {
        calls(&[(
            "agent-1",
            "agent",
            json!({"description": "Find the greeting", "prompt": "Where does a.txt say hello?", "agent_type": agent_type}),
        )])
    }

    type Requests = Arc<Mutex<Vec<Value>>>;

    fn scripted(
        root: &std::path::Path,
        responses: Vec<Result<ProviderResponse, ProviderError>>,
    ) -> (AgentService, Requests) {
        let provider = ScriptedProvider::new(responses);
        let requests = provider.recorded_requests.clone();
        let service = AgentService::with_provider(
            root.into(),
            Box::new(provider),
            "test".into(),
            1000,
            reqwest::Client::new(),
        );
        (service, requests)
    }

    fn tool_names(request: &Value) -> Vec<String> {
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// The `tool_result` blocks of the newest message of a request.
    fn last_results(request: &Value) -> Vec<Value> {
        let messages = request["messages"].as_array().unwrap();
        messages.last().unwrap()["content"]
            .as_array()
            .unwrap()
            .clone()
    }

    #[tokio::test]
    async fn explorer_works_in_a_fresh_context_and_only_its_report_comes_back() {
        let workspace = make_temp_dir("subagent_explorer");
        std::fs::write(workspace.path().join("a.txt"), "hello\n").unwrap();
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                delegate("explorer"),
                calls(&[("read-1", "read_file", json!({"path": "a.txt"}))]),
                Ok(text_provider_response(
                    "a.txt:1 says hello",
                    Some(TokenUsage {
                        input_tokens: 900,
                        output_tokens: 40,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 100,
                    }),
                )),
                text("It is in a.txt."),
            ],
        );
        let mut progress = Vec::new();

        let reply = service
            .submit_with_progress("Find the greeting".into(), |event| progress.push(event))
            .await
            .unwrap();

        assert_eq!(reply.text, "It is in a.txt.");
        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 4);
        // The main agent offers `agent`; the explorer gets read-only tools.
        assert!(tool_names(&requests[0]).contains(&"agent".to_string()));
        assert_eq!(requests[0]["system"], DEFAULT_SYSTEM_PROMPT);
        let child = &requests[1];
        assert_eq!(
            child["system"],
            format!("{SUBAGENT_FRAMING}\n\n{SUBAGENT_EXPLORER_INSTRUCTIONS}")
        );
        assert_eq!(
            child["messages"],
            json!([{"role": "user", "content": "Where does a.txt say hello?"}])
        );
        assert_eq!(
            tool_names(child),
            [
                "read_file",
                "list_files",
                "search_text",
                "git_branch",
                "git_status",
                "git_diff",
                "git_log",
                "find_symbol"
            ]
        );
        assert_eq!(last_results(&requests[2])[0]["tool_use_id"], "read-1");
        // The main conversation sees the report and none of the subagent's
        // own calls.
        let main = &requests[3];
        assert_eq!(main["system"], DEFAULT_SYSTEM_PROMPT);
        let results = last_results(main);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["tool_use_id"], "agent-1");
        assert_eq!(results[0].get("is_error"), None);
        // The report, then what the run cost; usage the first round did not
        // report is left out of the total.
        let content = results[0]["content"].as_str().unwrap();
        assert!(
            content.starts_with(
                "a.txt:1 says hello\n\n(subagent agent-1: 2 rounds, 1 tool call, 1000 input + 40 output tokens, "
            ),
            "{content}"
        );
        assert!(content.ends_with("s)"), "{content}");
        assert!(!main.to_string().contains("read-1"));
        assert!(
            !service
                .transcript_entries()
                .iter()
                .any(|entry| entry.text.contains("read-1"))
        );

        // Only the main agent's text is streamed as the reply.
        let streamed = progress
            .iter()
            .filter_map(|event| match event {
                AgentProgress::AssistantText { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(streamed, ["It is in a.txt."]);
        // Subagent activity is tagged with the call that started it.
        let activity = progress
            .iter()
            .filter_map(|event| match event {
                AgentProgress::Activity(activity) => Some(activity),
                _ => None,
            })
            .collect::<Vec<_>>();
        let read = activity
            .iter()
            .find(|event| matches!(&event.kind, ActivityKind::ToolStarted { name, .. } if name == "read_file"))
            .unwrap();
        assert_eq!(read.agent.as_deref(), Some("agent-1"));
        assert_eq!(read.round, 1);
        let delegated = activity
            .iter()
            .find(|event| matches!(&event.kind, ActivityKind::ToolStarted { name, .. } if name == "agent"))
            .unwrap();
        assert_eq!(delegated.agent, None);
        assert!(matches!(
            &delegated.kind,
            ActivityKind::ToolStarted { input_summary, .. } if input_summary == "Find the greeting"
        ));
        assert_eq!(
            activity
                .iter()
                .filter(|event| event.agent.is_some()
                    && matches!(event.kind, ActivityKind::RoundStarted { .. }))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn explorer_is_refused_tools_outside_its_type_and_cannot_nest() {
        let workspace = make_temp_dir("subagent_refused");
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                delegate("explorer"),
                calls(&[
                    (
                        "patch-1",
                        "apply_patch",
                        json!({"patch": "*** Begin Patch\n*** Add File: new.txt\n+x\n*** End Patch"}),
                    ),
                    (
                        "nested-1",
                        "agent",
                        json!({"description": "d", "prompt": "p", "agent_type": "explorer"}),
                    ),
                ]),
                text("I could not edit."),
                text("Done."),
            ],
        );

        service.submit("Try it".into()).await.unwrap();

        assert!(!workspace.path().join("new.txt").exists());
        let requests = requests.lock().unwrap().clone();
        // No request ran a nested subagent: main, child, child, main.
        assert_eq!(requests.len(), 4);
        let refused = last_results(&requests[2]);
        assert_eq!(refused.len(), 2);
        for (result, tool) in refused.iter().zip(["apply_patch", "agent"]) {
            assert_eq!(result["is_error"], true);
            assert_eq!(result["content"], format!("Unsupported tool: {tool}"));
        }
        // Refused calls never ran, so the report counts none.
        let report = &last_results(&requests[3])[0]["content"];
        assert!(
            report
                .as_str()
                .unwrap()
                .contains("(subagent agent-1: 2 rounds, 0 tool calls, "),
            "{report}"
        );
    }

    #[tokio::test]
    async fn worker_gets_every_tool_but_agent() {
        let workspace = make_temp_dir("subagent_worker");
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                delegate("worker"),
                text("Nothing to change."),
                text("Done."),
            ],
        );

        service.submit("Fix it".into()).await.unwrap();

        let requests = requests.lock().unwrap().clone();
        let mut expected = tool_names(&requests[0]);
        expected.retain(|name| !MAIN_AGENT_TOOLS.contains(&name.as_str()));
        assert_eq!(
            requests[1]["system"],
            format!("{SUBAGENT_FRAMING}\n\n{SUBAGENT_WORKER_INSTRUCTIONS}")
        );
        assert_eq!(tool_names(&requests[1]), expected);
    }

    #[tokio::test]
    async fn a_workers_edits_belong_to_the_turn_that_started_it() {
        let workspace = make_temp_dir("subagent_edit_origin");
        std::fs::write(workspace.path().join("a.txt"), "hello\n").unwrap();
        let (mut service, _) = scripted(
            workspace.path(),
            vec![
                delegate("worker"),
                calls(&[(
                    "patch-1",
                    "apply_patch",
                    json!({"patch": "*** Begin Patch\n*** Update File: a.txt\n@@\n-hello\n+bye\n*** End Patch"}),
                )]),
                text("Edited."),
                text("Done."),
            ],
        );

        service.submit("Change the greeting".into()).await.unwrap();

        assert_eq!(service.rewind_points()[0].changed_files, ["a.txt"]);
        let rewound = service.rewind(0, crate::RewindMode::Files).unwrap();
        assert_eq!(rewound.restored_files, ["a.txt"]);
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("a.txt")).unwrap(),
            "hello\n"
        );
    }

    #[tokio::test]
    async fn a_failed_subagent_is_an_error_result_and_the_main_agent_continues() {
        let workspace = make_temp_dir("subagent_failed");
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                delegate("worker"),
                Err(ProviderError::Api("overloaded".into())),
                text("The subagent failed; I will look myself."),
            ],
        );

        let reply = service.submit("Fix it".into()).await.unwrap();

        assert_eq!(reply.text, "The subagent failed; I will look myself.");
        let requests = requests.lock().unwrap().clone();
        let result = &last_results(&requests[2])[0];
        assert_eq!(result["is_error"], true);
        let content = result["content"].as_str().unwrap();
        assert!(content.starts_with("The subagent failed: "), "{content}");
        assert!(content.contains("overloaded"), "{content}");
        assert!(content.contains("Changes it made before stopping are kept"));
        assert!(
            content
                .contains("(subagent agent-1: 1 round, 0 tool calls, token usage not reported, "),
            "{content}"
        );
    }

    /// A subagent model with a window too small for a long prompt.
    fn tiny_window_model() -> (super::catalog::SubagentModel, Arc<Mutex<Vec<Value>>>) {
        let provider = ScriptedProvider::new(Vec::new());
        let requests = provider.recorded_requests.clone();
        let mut config = crate::models::ModelRegistry::builtin()
            .find("haiku")
            .unwrap()
            .clone();
        config.alias = "tiny".into();
        config.context_window = Some(4_000);
        let model = super::catalog::SubagentModel {
            config,
            provider: Box::new(provider),
            max_tokens: 1000,
        };
        (model, requests)
    }

    #[tokio::test]
    async fn a_subagent_whose_window_fills_reports_it_in_one_sentence() {
        let workspace = make_temp_dir("subagent_context_full");
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                calls(&[(
                    "agent-1",
                    "agent",
                    json!({"description": "d", "prompt": "word ".repeat(20_000), "agent_type": "explorer", "model": "tiny"}),
                )]),
                text("Done."),
            ],
        );
        let (tiny, tiny_requests) = tiny_window_model();
        service.install_subagents(super::catalog::SubagentCatalog::new(Vec::new(), vec![tiny]));

        service.submit("Delegate".into()).await.unwrap();

        assert!(tiny_requests.lock().unwrap().is_empty());
        let result = &last_results(&requests.lock().unwrap()[1])[0];
        assert_eq!(result["is_error"], true);
        let content = result["content"].as_str().unwrap();
        assert!(
            content.starts_with("The subagent failed: The latest message alone is about "),
            "{content}"
        );
        assert!(
            content.contains("message.\n\n(subagent agent-1: 0 rounds, 0 tool calls, "),
            "{content}"
        );
        assert!(!content.contains(".."), "{content}");
    }

    #[tokio::test]
    async fn invalid_input_is_refused_before_any_model_call() {
        let workspace = make_temp_dir("subagent_invalid");
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                calls(&[
                    (
                        "bad-type",
                        "agent",
                        json!({"description": "d", "prompt": "p", "agent_type": "admin"}),
                    ),
                    (
                        "no-prompt",
                        "agent",
                        json!({"description": "d", "prompt": "  ", "agent_type": "explorer"}),
                    ),
                    (
                        "no-description",
                        "agent",
                        json!({"description": " ", "prompt": "p", "agent_type": "explorer"}),
                    ),
                    (
                        "no-models",
                        "agent",
                        json!({"description": "d", "prompt": "p", "agent_type": "explorer", "model": "haiku"}),
                    ),
                ]),
                text("Done."),
            ],
        );

        service.submit("Delegate".into()).await.unwrap();

        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        let results = last_results(&requests[1]);
        assert_eq!(
            results[0]["content"],
            "Unknown agent_type 'admin'; use one of: explorer, worker"
        );
        assert_eq!(results[1]["content"], "agent requires a non-empty prompt");
        assert_eq!(
            results[2]["content"],
            "agent requires a non-empty description"
        );
        assert_eq!(
            results[3]["content"],
            "Model 'haiku' is not available to subagents, and no other model is; leave model out to run on the current one"
        );
    }

    #[tokio::test]
    async fn null_optional_fields_mean_not_given() {
        let workspace = make_temp_dir("subagent_null_fields");
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                calls(&[(
                    "agent-1",
                    "agent",
                    json!({"description": "d", "prompt": "p", "agent_type": "explorer", "model": null, "resume": null}),
                )]),
                text("Nothing here."),
                text("Done."),
            ],
        );

        service.submit("Delegate".into()).await.unwrap();

        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 3, "the subagent ran on the current model");
        let result = &last_results(&requests[2])[0];
        assert_eq!(result.get("is_error"), None, "{result}");
        assert!(
            result["content"]
                .as_str()
                .unwrap()
                .starts_with("Nothing here.\n\n(subagent agent-1: 1 round")
        );
    }

    #[tokio::test]
    async fn approvals_a_worker_asks_for_name_the_subagent() {
        let workspace = make_temp_dir("subagent_approval");
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                delegate("worker"),
                calls(&[(
                    "cmd-1",
                    "run_command",
                    json!({"command": ["cargo", "test"]}),
                )]),
                text("The test command was denied."),
                text("Done."),
            ],
        );
        let control = service.task_control();
        control.enable_interactive_approval();
        let mut asked = Vec::new();

        service
            .submit_with_progress("Run the tests".into(), |event| {
                if let AgentProgress::ApprovalRequested {
                    request_id,
                    summary,
                    requested_by,
                    ..
                } = event
                {
                    asked.push((summary, requested_by));
                    control.answer_approval(request_id, false);
                }
            })
            .await
            .unwrap();

        assert_eq!(
            asked,
            [(
                "cargo test".to_string(),
                Some("worker subagent \"Find the greeting\"".to_string())
            )]
        );
        let denied = &last_results(&requests.lock().unwrap()[2])[0];
        assert_eq!(denied["is_error"], true);
        assert!(
            denied["content"]
                .as_str()
                .unwrap()
                .starts_with("Denied by the user")
        );
        let report = &last_results(&requests.lock().unwrap()[3])[0]["content"];
        assert!(
            report
                .as_str()
                .unwrap()
                .contains("(subagent agent-1: 2 rounds, 0 tool calls, "),
            "{report}"
        );
    }

    #[tokio::test]
    async fn the_subagent_transcript_marks_calls_the_user_approved() {
        let workspace = make_temp_dir("subagent_approved_transcript");
        let (mut service, _) = scripted(
            workspace.path(),
            vec![
                delegate("worker"),
                calls(&[
                    ("cmd-1", "run_command", json!({"command": ["echo", "hi"]})),
                    ("read-1", "read_file", json!({"path": "a.txt"})),
                ]),
                text("Ran it."),
                text("Done."),
            ],
        );
        std::fs::write(workspace.path().join("a.txt"), "hello\n").unwrap();
        let control = service.task_control();
        control.enable_interactive_approval();

        service
            .submit_with_progress("Run it".into(), |event| {
                if let AgentProgress::ApprovalRequested { request_id, .. } = event {
                    control.answer_approval(request_id, true);
                }
            })
            .await
            .unwrap();

        let (_, transcript) = service.subagent_transcript("agent-1").unwrap();
        let tools = transcript
            .iter()
            .filter(|entry| matches!(entry.role, crate::TranscriptRole::Tool { .. }))
            .map(|entry| entry.text.lines().next().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(tools, ["run_command echo hi (approved)", "read_file a.txt"]);
    }

    #[tokio::test]
    async fn a_user_type_runs_with_its_own_prompt_tools_and_model() {
        use super::catalog::{SubagentCatalog, SubagentModel, SubagentType, ToolSet};
        let workspace = make_temp_dir("subagent_user_type");
        let (mut service, main_requests) =
            scripted(workspace.path(), vec![delegate("reviewer"), text("Done.")]);
        let model_provider = ScriptedProvider::new(vec![text("No bugs found.")]);
        let model_requests = model_provider.recorded_requests.clone();
        let haiku = crate::models::ModelRegistry::builtin()
            .find("haiku")
            .unwrap()
            .clone();
        service.install_subagents(SubagentCatalog::new(
            vec![SubagentType {
                name: "reviewer".into(),
                description: "Reviews a diff".into(),
                instructions: "You review.".into(),
                tools: ToolSet::Only(vec!["git_diff".into(), "read_file".into()]),
                model: Some("haiku".into()),
            }],
            vec![SubagentModel {
                config: haiku,
                provider: Box::new(model_provider),
                max_tokens: 321,
            }],
        ));

        service.submit("Review it".into()).await.unwrap();

        let main_requests = main_requests.lock().unwrap().clone();
        assert_eq!(
            main_requests.len(),
            2,
            "the main model ran no subagent round"
        );
        let agent = main_requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "agent")
            .unwrap()
            .clone();
        assert_eq!(
            agent["input_schema"]["properties"]["agent_type"]["enum"],
            json!(["explorer", "worker", "reviewer"])
        );
        let child = model_requests.lock().unwrap()[0].clone();
        assert_eq!(child["model"], "claude-haiku-5-5");
        assert_eq!(child["max_tokens"], 321);
        // The user writes only the role; the framing every subagent needs
        // comes first.
        let system = child["system"].as_str().unwrap();
        assert!(
            system.starts_with("You are a subagent of Socium")
                && system.contains(
                    "you cannot see its conversation, and it sees only your final message."
                ),
            "{system}"
        );
        assert!(system.ends_with("\n\nYou review."), "{system}");
        assert_eq!(tool_names(&child), ["read_file", "git_diff"]);
        let report = last_results(&main_requests[1])[0]["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            report.starts_with("No bugs found.\n\n(subagent agent-1: 1 round"),
            "{report}"
        );
    }

    #[tokio::test]
    async fn a_call_can_choose_a_model_and_unknown_models_are_refused() {
        use super::catalog::{SubagentCatalog, SubagentModel};
        let workspace = make_temp_dir("subagent_call_model");
        let (mut service, main_requests) = scripted(
            workspace.path(),
            vec![
                calls(&[
                    (
                        "agent-1",
                        "agent",
                        json!({"description": "d", "prompt": "Look", "agent_type": "explorer", "model": "haiku"}),
                    ),
                    (
                        "agent-2",
                        "agent",
                        json!({"description": "d", "prompt": "Look", "agent_type": "explorer", "model": "gpt"}),
                    ),
                ]),
                text("Done."),
            ],
        );
        let haiku_provider = ScriptedProvider::new(vec![text("Found it.")]);
        let haiku_requests = haiku_provider.recorded_requests.clone();
        let haiku = crate::models::ModelRegistry::builtin()
            .find("haiku")
            .unwrap()
            .clone();
        service.install_subagents(SubagentCatalog::new(
            Vec::new(),
            vec![SubagentModel {
                config: haiku,
                provider: Box::new(haiku_provider),
                max_tokens: 500,
            }],
        ));

        service.submit("Look".into()).await.unwrap();

        let child = haiku_requests.lock().unwrap()[0].clone();
        assert_eq!(child["model"], "claude-haiku-5-5");
        assert_eq!(
            child["system"],
            format!("{SUBAGENT_FRAMING}\n\n{SUBAGENT_EXPLORER_INSTRUCTIONS}")
        );
        let main_requests = main_requests.lock().unwrap().clone();
        assert_eq!(main_requests.len(), 2);
        let results = last_results(&main_requests[1]);
        assert!(
            results[0]["content"]
                .as_str()
                .unwrap()
                .starts_with("Found it.")
        );
        assert_eq!(results[1]["is_error"], true);
        assert_eq!(
            results[1]["content"],
            "Model 'gpt' is not available to subagents; use one of: haiku"
        );
    }

    fn record_path(service: &AgentService, id: &str) -> std::path::PathBuf {
        crate::session::session_file(
            &service.workspace_root,
            service.session_id(),
            &format!("agents/{id}.json"),
        )
    }

    fn record(service: &AgentService, id: &str) -> Value {
        let path = record_path(service, id);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn resume(id: &str, agent_type: &str, prompt: &str) -> Result<ProviderResponse, ProviderError> {
        calls(&[(
            "agent-2",
            "agent",
            json!({"description": "Follow up", "prompt": prompt, "agent_type": agent_type, "resume": id}),
        )])
    }

    #[tokio::test]
    async fn a_subagent_is_saved_and_resume_continues_it_with_everything_it_saw() {
        let workspace = make_temp_dir("subagent_resume");
        std::fs::write(workspace.path().join("a.txt"), "hello\nworld\n").unwrap();
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                delegate("explorer"),
                calls(&[("read-1", "read_file", json!({"path": "a.txt"}))]),
                text("a.txt:1 says hello"),
                text("Found it."),
                resume("agent-1", "explorer", "And line 2?"),
                text("a.txt:2 says world"),
                text("Line 2 says world."),
            ],
        );

        service.submit("Find the greeting".into()).await.unwrap();
        let saved = record(&service, "agent-1");
        assert_eq!(saved["status"], "finished");
        assert_eq!(saved["runs"], 1);
        assert_eq!(saved["agent_type"], "explorer");
        assert_eq!(saved["description"], "Find the greeting");
        assert_eq!(saved["messages"].as_array().unwrap().len(), 4);

        service.submit("And the next line?".into()).await.unwrap();

        let requests = requests.lock().unwrap().clone();
        // The resumed subagent got its earlier work back, then the prompt.
        let child = &requests[5];
        let messages = child["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0]["content"], "Where does a.txt say hello?");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "read-1");
        assert_eq!(messages[4]["content"], "And line 2?");
        let report = last_results(&requests[6])[0]["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            report.starts_with("a.txt:2 says world\n\n(subagent agent-1: 1 round, "),
            "{report}"
        );
        let saved = record(&service, "agent-1");
        assert_eq!(saved["runs"], 2);
        assert_eq!(saved["messages"].as_array().unwrap().len(), 6);

        let runs = service.subagent_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "agent-1");
        assert_eq!(runs[0].runs, 2);
        assert_eq!(runs[0].status, crate::SubagentStatus::Finished);
        let (summary, transcript) = service.subagent_transcript("agent-1").unwrap();
        assert_eq!(summary.description, "Find the greeting");
        let texts = transcript
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            texts,
            [
                "Where does a.txt say hello?",
                "read_file a.txt\nPath: a.txt (lines 1-2 of 2)",
                "a.txt:1 says hello",
                "And line 2?",
                "a.txt:2 says world",
            ]
        );
    }

    #[tokio::test]
    async fn resume_refuses_unknown_ids_path_like_ids_and_another_type() {
        let workspace = make_temp_dir("subagent_resume_refused");
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                delegate("explorer"),
                text("Report."),
                text("Done."),
                calls(&[
                    (
                        "r1",
                        "agent",
                        json!({"description": "d", "prompt": "p", "agent_type": "explorer", "resume": "missing"}),
                    ),
                    (
                        "r2",
                        "agent",
                        json!({"description": "d", "prompt": "p", "agent_type": "explorer", "resume": "../agent-1"}),
                    ),
                    (
                        "r3",
                        "agent",
                        json!({"description": "d", "prompt": "p", "agent_type": "worker", "resume": "agent-1"}),
                    ),
                ]),
                text("Done."),
            ],
        );
        service.submit("First".into()).await.unwrap();

        service.submit("Again".into()).await.unwrap();

        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 5, "no subagent ran for the refused calls");
        let results = last_results(&requests[4]);
        let contents = results
            .iter()
            .map(|result| result["content"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            contents,
            [
                "No saved subagent 'missing' in this session; only subagents started in this session can be resumed",
                "Subagent record: '../agent-1' is not a subagent ID",
                "Subagent 'agent-1' has agent_type 'explorer'; resume it with that type",
            ]
        );
    }

    /// Paths of the files under `dir` that contain `needle`.
    fn files_containing(dir: &std::path::Path, needle: &str) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.extend(files_containing(&path, needle));
            } else if std::fs::read_to_string(&path).is_ok_and(|text| text.contains(needle)) {
                found.push(path);
            }
        }
        found
    }

    #[tokio::test]
    async fn subagents_belong_to_their_session_and_go_with_it() {
        let workspace = make_temp_dir("subagent_sessions");
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                delegate("explorer"),
                text("a.txt:1 says hello"),
                text("Found it."),
                resume("agent-1", "explorer", "And line 2?"),
                text("I cannot resume it here."),
            ],
        );
        service.submit("Find the greeting".into()).await.unwrap();
        let first = service.session_id().to_string();
        assert_eq!(service.subagent_runs().unwrap().len(), 1);

        service.clear().unwrap();

        // A new session lists none of the old one's subagents and cannot
        // resume them.
        assert!(service.subagent_runs().unwrap().is_empty());
        assert!(service.subagent_transcript("agent-1").is_err());
        service.submit("Ask it again".into()).await.unwrap();
        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 5, "no subagent ran for the refused resume");
        assert_eq!(
            last_results(&requests[4])[0]["content"],
            "No saved subagent 'agent-1' in this session; only subagents started in this session can be resumed"
        );

        // Deleting the session deletes what its subagents were told.
        service.delete_session(&first).unwrap();
        assert_eq!(
            files_containing(workspace.path(), "Where does a.txt say hello?"),
            Vec::<std::path::PathBuf>::new()
        );
    }

    #[tokio::test]
    async fn a_failed_run_is_saved_with_its_tool_results_and_resume_repairs_a_cut_batch() {
        let workspace = make_temp_dir("subagent_saved_failure");
        std::fs::write(workspace.path().join("a.txt"), "hello\n").unwrap();
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                delegate("explorer"),
                calls(&[("read-1", "read_file", json!({"path": "a.txt"}))]),
                Err(ProviderError::Api("overloaded".into())),
                text("It failed."),
            ],
        );

        service.submit("Find it".into()).await.unwrap();

        let saved = record(&service, "agent-1");
        assert_eq!(saved["status"], "failed");
        let messages = saved["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "read-1");
        assert_eq!(requests.lock().unwrap().len(), 4);
        let path = record_path(&service, "agent-1");
        drop(service);

        // A crash between the call and its result leaves the call alone.
        let mut cut = saved.clone();
        cut["messages"].as_array_mut().unwrap().truncate(2);
        cut["status"] = json!("running");
        std::fs::write(&path, serde_json::to_string(&cut).unwrap()).unwrap();
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                resume("agent-1", "explorer", "Try again"),
                text("Recovered."),
                text("Done."),
            ],
        );
        service.restore_state().unwrap();

        service.submit("Again".into()).await.unwrap();

        let child = requests.lock().unwrap()[1].clone();
        let messages = child["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        let repaired = &messages[2]["content"][0];
        assert_eq!(repaired["tool_use_id"], "read-1");
        assert_eq!(repaired["is_error"], true);
        assert!(
            repaired["content"]
                .as_str()
                .unwrap()
                .starts_with("Interrupted before result was saved")
        );
        assert_eq!(messages[3]["content"], "Try again");
    }

    /// The `sonnet` registry entry with a small window, so a few reads
    /// fill it; it summarizes with `haiku` as `sonnet` does.
    fn small_window_models(
        child: Vec<Result<ProviderResponse, ProviderError>>,
        summarizer: Vec<Result<ProviderResponse, ProviderError>>,
    ) -> (super::catalog::SubagentCatalog, Requests, Requests) {
        use super::catalog::{SubagentCatalog, SubagentModel};
        let registry = crate::models::ModelRegistry::builtin();
        let mut small = registry.find("sonnet").unwrap().clone();
        small.alias = "small".into();
        small.context_window = Some(36_000);
        let child = ScriptedProvider::new(child);
        let child_requests = child.recorded_requests.clone();
        let summarizer = ScriptedProvider::new(summarizer);
        let summary_requests = summarizer.recorded_requests.clone();
        let catalog = SubagentCatalog::new(
            Vec::new(),
            vec![
                SubagentModel {
                    config: small,
                    provider: Box::new(child),
                    max_tokens: 1000,
                },
                SubagentModel {
                    config: registry.find("haiku").unwrap().clone(),
                    provider: Box::new(summarizer),
                    max_tokens: 1000,
                },
            ],
        );
        (catalog, child_requests, summary_requests)
    }

    /// Characters per token the estimate uses for Claude model IDs.
    fn estimated_tokens(request: &Value) -> usize {
        (request.to_string().chars().count() as f32 / 3.5) as usize
    }

    #[tokio::test]
    async fn a_subagent_whose_window_fills_summarizes_its_history_and_still_reports() {
        let workspace = make_temp_dir("subagent_window_fills");
        std::fs::write(
            workspace.path().join("big.txt"),
            format!("{}\n", "w".repeat(99)).repeat(1_000),
        )
        .unwrap();
        let read = |id: &str| (id.to_string(), json!({"path": "big.txt"}));
        let reads = |ids: &[&str]| {
            let batch = ids.iter().map(|id| read(id)).collect::<Vec<_>>();
            calls(
                &batch
                    .iter()
                    .map(|(id, input)| (id.as_str(), "read_file", input.clone()))
                    .collect::<Vec<_>>(),
            )
        };
        // The first two reads fit once the newest is cut; the third round's
        // results cannot be cut far enough, so the window fills.
        let (catalog, child_requests, summary_requests) = small_window_models(
            vec![
                reads(&["read-1"]),
                reads(&["read-2"]),
                reads(&["read-3", "read-4", "read-5"]),
                text("big.txt is 1000 lines of w"),
            ],
            vec![text("1. User request: read big.txt")],
        );
        let (mut service, main_requests) = scripted(
            workspace.path(),
            vec![
                calls(&[(
                    "agent-1",
                    "agent",
                    json!({"description": "Read it", "prompt": "Read big.txt and describe it", "agent_type": "explorer", "model": "small"}),
                )]),
                text("Done."),
            ],
        );
        service.install_subagents(catalog);
        let mut activity = Vec::new();

        service
            .submit_with_progress("Describe big.txt".into(), |event| {
                if let AgentProgress::Activity(event) = event {
                    activity.push(event);
                }
            })
            .await
            .unwrap();

        let result = &last_results(&main_requests.lock().unwrap()[1])[0];
        assert_eq!(result.get("is_error"), None, "{result}");
        let report = result["content"].as_str().unwrap();
        assert!(
            report.starts_with(
                "big.txt is 1000 lines of w\n\n(subagent agent-1: 4 rounds, 5 tool calls, "
            ),
            "{report}"
        );
        // The small model's own compaction model wrote the summary.
        let summaries = summary_requests.lock().unwrap().clone();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0]["model"], "claude-haiku-5-5");
        assert_eq!(
            summaries[0]["system"],
            crate::constants::COMPACTION_SYSTEM_PROMPT
        );
        assert!(
            summaries[0]["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("Read big.txt and describe it")
        );
        // Every request fit the window, and the one after the cut starts
        // from the summary with the open calls and their results.
        let child_requests = child_requests.lock().unwrap().clone();
        assert_eq!(child_requests.len(), 4);
        for request in &child_requests {
            assert!(
                estimated_tokens(request) < 36_000,
                "{}",
                estimated_tokens(request)
            );
        }
        let resumed = child_requests[3]["messages"].as_array().unwrap();
        assert_eq!(resumed.len(), 3);
        assert!(
            resumed[0]["content"]
                .as_str()
                .unwrap()
                .contains("1. User request: read big.txt")
        );
        assert_eq!(resumed[1]["content"][0]["id"], "read-3");
        assert_eq!(resumed[2]["content"][2]["tool_use_id"], "read-5");
        assert!(
            activity
                .iter()
                .any(|event| event.agent.as_deref() == Some("agent-1")
                    && event.kind
                        == ActivityKind::CompactionFinished {
                            covered_messages: 5,
                            used_llm: true
                        })
        );
        // `/agents` still shows the whole conversation.
        let (_, transcript) = service.subagent_transcript("agent-1").unwrap();
        assert_eq!(transcript[0].text, "Read big.txt and describe it");
    }

    #[tokio::test]
    async fn resuming_a_subagent_whose_history_no_longer_fits_summarizes_it_first() {
        use super::record::{SubagentRecord, SubagentStatus};
        use crate::agent::types::Message;
        let workspace = make_temp_dir("subagent_resume_full");
        let mut saved = SubagentRecord::new("agent-1".into(), "explorer", "d", "test");
        saved.messages = vec![
            Message::user_text(format!("Look at this: {}", "x".repeat(1_000_000))),
            Message::assistant_blocks(vec![ContentBlock::Text {
                text: "It is all x.".into(),
            }]),
        ];
        saved.status = SubagentStatus::Finished;
        saved.runs = 1;
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                resume("agent-1", "explorer", "How long is it?"),
                text("1. User request: look at a long run of x"),
                text("About a million characters."),
                text("Done."),
            ],
        );
        saved.save(workspace.path(), service.session_id()).unwrap();

        service.submit("How long?".into()).await.unwrap();

        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 4);
        // Without its own model, the subagent summarizes as the main agent
        // does.
        assert_eq!(requests[1]["model"], DEFAULT_COMPACTION_MODEL);
        assert_eq!(
            requests[1]["system"],
            crate::constants::COMPACTION_SYSTEM_PROMPT
        );
        let child = &requests[2];
        assert_eq!(
            child["system"],
            format!("{SUBAGENT_FRAMING}\n\n{SUBAGENT_EXPLORER_INSTRUCTIONS}")
        );
        let messages = child["messages"].as_array().unwrap();
        assert!(
            messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("1. User request: look at a long run of x")
        );
        assert_eq!(messages.last().unwrap()["content"], "How long is it?");
        assert!(estimated_tokens(child) < 200_000);
        let report = last_results(&requests[3])[0]["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            report.starts_with("About a million characters.\n\n"),
            "{report}"
        );
        assert_eq!(record(&service, "agent-1")["status"], "finished");
    }

    #[tokio::test]
    async fn a_resume_that_fails_before_any_answer_leaves_the_record_as_it_was() {
        let workspace = make_temp_dir("subagent_resume_unanswered");
        let (mut service, requests) = scripted(
            workspace.path(),
            vec![
                delegate("explorer"),
                text("Report."),
                text("Done."),
                resume("agent-1", "explorer", "First follow-up"),
                Err(ProviderError::Api("invalid request".into())),
                text("It failed."),
                resume("agent-1", "explorer", "Second follow-up"),
                text("Second report."),
                text("Done."),
            ],
        );
        service.submit("First".into()).await.unwrap();
        let before = record(&service, "agent-1");

        service.submit("Again".into()).await.unwrap();

        let failed = record(&service, "agent-1");
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["messages"], before["messages"]);
        assert_eq!(failed["runs"], 1);

        service.submit("Once more".into()).await.unwrap();

        let child = requests.lock().unwrap()[7].clone();
        let sent = child["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["content"].clone())
            .collect::<Vec<_>>();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[2], "Second follow-up");
        assert_eq!(record(&service, "agent-1")["runs"], 2);
    }

    /// Answers the main agent from a script. Each subagent request waits
    /// until `gate` has as many subagents waiting at once, then reports its
    /// prompt, so subagents run one at a time never get past it.
    struct GatedProvider {
        main: ScriptedProvider,
        gate: tokio::sync::Barrier,
    }

    #[async_trait::async_trait]
    impl crate::providers::Provider for GatedProvider {
        fn kind(&self) -> crate::providers::ProviderKind {
            crate::providers::ProviderKind::Anthropic
        }

        fn capabilities(&self) -> crate::providers::ProviderCapabilities {
            crate::providers::ProviderCapabilities {
                supports_tools: true,
            }
        }

        async fn send_message(
            &self,
            client: &reqwest::Client,
            request: &crate::agent::types::AgentMessageRequest<'_>,
            on_delta: &mut (dyn FnMut(crate::providers::ProviderEvent) + Send),
        ) -> Result<ProviderResponse, ProviderError> {
            let value = serde_json::to_value(request)?;
            if value["system"] == DEFAULT_SYSTEM_PROMPT {
                return self.main.send_message(client, request, on_delta).await;
            }
            self.gate.wait().await;
            let prompt = value["messages"][0]["content"].as_str().unwrap_or_default();
            Ok(text_provider_response(&format!("report on {prompt}"), None))
        }
    }

    #[tokio::test]
    async fn read_only_subagents_in_one_response_run_at_the_same_time() {
        let workspace = make_temp_dir("subagent_concurrent");
        let explore = |id: &str, prompt: &str| {
            (
                id.to_string(),
                json!({"description": "Look", "prompt": prompt, "agent_type": "explorer"}),
            )
        };
        let batch = [explore("agent-a", "src"), explore("agent-b", "docs")];
        let provider = GatedProvider {
            main: ScriptedProvider::new(vec![
                calls(
                    &batch
                        .iter()
                        .map(|(id, input)| (id.as_str(), "agent", input.clone()))
                        .collect::<Vec<_>>(),
                ),
                text("Both done."),
            ]),
            gate: tokio::sync::Barrier::new(2),
        };
        let requests = provider.main.recorded_requests.clone();
        let mut service = AgentService::with_provider(
            workspace.path().into(),
            Box::new(provider),
            "test".into(),
            1000,
            reqwest::Client::new(),
        );
        let mut finished = Vec::new();

        let reply = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            service.submit_with_progress("Look at both".into(), |event| {
                if let AgentProgress::Activity(activity) = event
                    && let ActivityKind::ToolFinished { call_id, .. } = activity.kind
                {
                    finished.push(call_id);
                }
            }),
        )
        .await
        .expect("the two subagents waited for each other, so they ran at once")
        .unwrap();

        assert_eq!(reply.text, "Both done.");
        finished.sort();
        assert_eq!(finished, ["agent-a", "agent-b"]);
        // Results come back in call order, each with its own record.
        let results = last_results(&requests.lock().unwrap()[1]);
        let contents = results
            .iter()
            .map(|result| {
                (
                    result["tool_use_id"].as_str().unwrap(),
                    result["content"].as_str().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(contents[0].0, "agent-a");
        assert!(
            contents[0]
                .1
                .starts_with("report on src\n\n(subagent agent-a: ")
        );
        assert_eq!(contents[1].0, "agent-b");
        assert!(
            contents[1]
                .1
                .starts_with("report on docs\n\n(subagent agent-b: ")
        );
        assert_eq!(record(&service, "agent-a")["status"], "finished");
        assert_eq!(record(&service, "agent-b")["status"], "finished");
    }

    /// The tool entries a turn shows live, then the ones a restarted
    /// session rebuilds from the saved conversation.
    async fn live_and_restored_tool_entries(
        root: &std::path::Path,
        responses: Vec<Result<ProviderResponse, ProviderError>>,
    ) -> (Vec<TranscriptEntry>, Vec<TranscriptEntry>) {
        let (mut service, _) = scripted(root, responses);
        let mut live = Vec::new();
        service
            .submit_with_progress("Look around".into(), |event| {
                if let AgentProgress::ToolCallRecorded(entry) = event {
                    live.push(entry);
                }
            })
            .await
            .unwrap();
        drop(service);
        let (mut restarted, _) = scripted(root, Vec::new());
        restarted.restore_state().unwrap();
        let restored = restarted
            .transcript_entries()
            .into_iter()
            .filter(|entry| matches!(entry.role, TranscriptRole::Tool { .. }))
            .collect();
        (live, restored)
    }

    #[tokio::test]
    async fn the_live_conversation_shows_agent_calls_as_a_restart_does() {
        let workspace = make_temp_dir("subagent_live_sequential");
        std::fs::write(workspace.path().join("a.txt"), "hello\n").unwrap();
        let (live, restored) = live_and_restored_tool_entries(
            workspace.path(),
            vec![
                delegate("explorer"),
                calls(&[("read-1", "read_file", json!({"path": "a.txt"}))]),
                text("a.txt:1 says hello"),
                text("It is in a.txt."),
            ],
        )
        .await;
        // The explorer's read belongs to the Activity pane.
        assert_eq!(live, restored);
        assert_eq!(restored.len(), 1, "{restored:?}");
        assert!(
            restored[0]
                .text
                .starts_with("agent Find the greeting\na.txt:1 says hello"),
            "{restored:?}"
        );

        let workspace = make_temp_dir("subagent_live_batch");
        let explore = |id, prompt| {
            (
                id,
                "agent",
                json!({"description": format!("Look at {prompt}"), "prompt": prompt, "agent_type": "explorer"}),
            )
        };
        let (live, restored) = live_and_restored_tool_entries(
            workspace.path(),
            vec![
                calls(&[explore("agent-a", "src"), explore("agent-b", "docs")]),
                text("report one"),
                text("report two"),
                text("Both done."),
            ],
        )
        .await;
        assert_eq!(live, restored);
        let firsts = restored
            .iter()
            .map(|entry| entry.text.lines().next().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(firsts, ["agent Look at src", "agent Look at docs"]);
    }

    fn explore(id: &str, prompt: &str) -> (String, Value) {
        (
            id.to_string(),
            json!({"description": "Look", "prompt": prompt, "agent_type": "explorer"}),
        )
    }

    fn delegate_all(batch: &[(String, Value)]) -> Result<ProviderResponse, ProviderError> {
        calls(
            &batch
                .iter()
                .map(|(id, input)| (id.as_str(), "agent", input.clone()))
                .collect::<Vec<_>>(),
        )
    }

    /// Answers the main agent from a script. A subagent asked for "slow"
    /// waits until two others have reported, then copies the conversation
    /// file to `crashed`, as the process dying at that moment would leave it.
    struct CrashingProvider {
        main: ScriptedProvider,
        reported: tokio::sync::Semaphore,
        conversation: Arc<std::sync::OnceLock<std::path::PathBuf>>,
        crashed: std::path::PathBuf,
    }

    #[async_trait::async_trait]
    impl crate::providers::Provider for CrashingProvider {
        fn kind(&self) -> crate::providers::ProviderKind {
            crate::providers::ProviderKind::Anthropic
        }

        fn capabilities(&self) -> crate::providers::ProviderCapabilities {
            crate::providers::ProviderCapabilities {
                supports_tools: true,
            }
        }

        async fn send_message(
            &self,
            client: &reqwest::Client,
            request: &crate::agent::types::AgentMessageRequest<'_>,
            on_delta: &mut (dyn FnMut(crate::providers::ProviderEvent) + Send),
        ) -> Result<ProviderResponse, ProviderError> {
            let value = serde_json::to_value(request)?;
            if value["system"] == DEFAULT_SYSTEM_PROMPT {
                return self.main.send_message(client, request, on_delta).await;
            }
            let prompt = value["messages"][0]["content"].as_str().unwrap_or_default();
            if prompt == "slow" {
                let _ = self.reported.acquire_many(2).await;
                std::fs::copy(self.conversation.get().unwrap(), &self.crashed).unwrap();
            } else {
                self.reported.add_permits(1);
            }
            Ok(text_provider_response(&format!("report on {prompt}"), None))
        }
    }

    #[tokio::test]
    async fn a_restart_keeps_the_reports_of_batched_subagents_that_finished() {
        let workspace = make_temp_dir("subagent_batch_restart");
        let conversation = Arc::new(std::sync::OnceLock::new());
        let crashed = workspace.path().join("crashed.json");
        let provider = CrashingProvider {
            main: ScriptedProvider::new(vec![
                delegate_all(&[
                    explore("agent-a", "src"),
                    explore("agent-b", "slow"),
                    explore("agent-c", "docs"),
                ]),
                text("All done."),
            ]),
            reported: tokio::sync::Semaphore::new(0),
            conversation: conversation.clone(),
            crashed: crashed.clone(),
        };
        let mut service = AgentService::with_provider(
            workspace.path().into(),
            Box::new(provider),
            "test".into(),
            1000,
            reqwest::Client::new(),
        );
        conversation.set(service.conversation_path()).unwrap();
        service.submit("Look around".into()).await.unwrap();
        drop(service);
        std::fs::copy(&crashed, conversation.get().unwrap()).unwrap();
        let (mut restarted, requests) = scripted(workspace.path(), vec![text("Resumed.")]);
        restarted.restore_state().unwrap();

        restarted.submit("Go on".into()).await.unwrap();

        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1, "no subagent ran again");
        let messages = requests[0]["messages"].as_array().unwrap();
        let results = messages[messages.len() - 2]["content"].as_array().unwrap();
        let ids = results
            .iter()
            .map(|result| result["tool_use_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["agent-a", "agent-b", "agent-c"]);
        let content = |index: usize| results[index]["content"].as_str().unwrap();
        assert!(
            content(0).starts_with("report on src\n\n(subagent agent-a: "),
            "{}",
            content(0)
        );
        assert_eq!(results[0].get("is_error"), None);
        assert_eq!(results[1]["is_error"], true);
        assert!(
            content(1).starts_with("Interrupted before result was saved"),
            "{}",
            content(1)
        );
        assert!(
            content(2).starts_with("report on docs\n\n(subagent agent-c: "),
            "{}",
            content(2)
        );
    }

    /// Answers the main agent from a script. A subagent searches the path
    /// its prompt names for "needle", then reports what the search returned.
    struct SearchingProvider {
        main: ScriptedProvider,
    }

    #[async_trait::async_trait]
    impl crate::providers::Provider for SearchingProvider {
        fn kind(&self) -> crate::providers::ProviderKind {
            crate::providers::ProviderKind::Anthropic
        }

        fn capabilities(&self) -> crate::providers::ProviderCapabilities {
            crate::providers::ProviderCapabilities {
                supports_tools: true,
            }
        }

        async fn send_message(
            &self,
            client: &reqwest::Client,
            request: &crate::agent::types::AgentMessageRequest<'_>,
            on_delta: &mut (dyn FnMut(crate::providers::ProviderEvent) + Send),
        ) -> Result<ProviderResponse, ProviderError> {
            let value = serde_json::to_value(request)?;
            if value["system"] == DEFAULT_SYSTEM_PROMPT {
                return self.main.send_message(client, request, on_delta).await;
            }
            let messages = value["messages"].as_array().unwrap();
            if messages.len() == 1 {
                return calls(&[(
                    "search",
                    "search_text",
                    json!({"pattern": "needle", "path": messages[0]["content"]}),
                )]);
            }
            let found = last_results(&value)[0]["content"]
                .as_str()
                .unwrap()
                .to_string();
            Ok(text_provider_response(&found, None))
        }
    }

    #[tokio::test]
    async fn the_tools_of_batched_subagents_run_at_the_same_time() {
        use std::io::Write;
        let workspace = make_temp_dir("subagent_concurrent_tools");
        let pipes = ["a.pipe", "b.pipe"].map(|name| workspace.path().join(name));
        for pipe in &pipes {
            let made = std::process::Command::new("mkfifo").arg(pipe).status();
            assert!(made.unwrap().success());
        }
        // Each subagent searches its own pipe. Opening a pipe to write waits
        // for its reader, so the match is written only once both searches
        // are running.
        let writer = std::thread::spawn(move || {
            let opened =
                pipes.map(|pipe| std::fs::OpenOptions::new().write(true).open(pipe).unwrap());
            for mut pipe in opened {
                let _ = pipe.write_all(b"needle\n");
            }
        });
        let provider = SearchingProvider {
            main: ScriptedProvider::new(vec![
                delegate_all(&[explore("agent-a", "a.pipe"), explore("agent-b", "b.pipe")]),
                text("Both searched."),
            ]),
        };
        let requests = provider.main.recorded_requests.clone();
        let mut service = AgentService::with_provider(
            workspace.path().into(),
            Box::new(provider),
            "test".into(),
            1000,
            reqwest::Client::new(),
        );
        let mut seen = Vec::new();

        service
            .submit_with_progress("Search both".into(), |event| {
                if let AgentProgress::Activity(activity) = event
                    && let Some(agent) = activity.agent
                    && let ActivityKind::ToolStarted { .. } | ActivityKind::ToolFinished { .. } =
                        activity.kind
                {
                    seen.push((agent, std::time::Instant::now()));
                }
            })
            .await
            .unwrap();

        let results = last_results(&requests.lock().unwrap()[1]);
        for (result, pipe) in results.iter().zip(["a.pipe", "b.pipe"]) {
            let report = result["content"].as_str().unwrap();
            assert!(report.contains(&format!("{pipe}:1:needle")), "{report}");
        }
        let span = |agent: &str| {
            let times = seen
                .iter()
                .filter(|(id, _)| id == agent)
                .map(|(_, at)| *at)
                .collect::<Vec<_>>();
            assert_eq!(times.len(), 2, "one search by {agent}");
            (times[0], times[1])
        };
        let (a, b) = (span("agent-a"), span("agent-b"));
        assert!(
            a.0 < b.1 && b.0 < a.1,
            "the searches ran one after the other"
        );
        writer.join().unwrap();
    }

    #[test]
    fn long_reports_are_cut_on_a_character_boundary() {
        let report = "é".repeat(super::MAX_SUBAGENT_REPORT_CHARACTERS + 5);

        let bounded = super::bounded_report(report);

        assert!(bounded.starts_with(&"é".repeat(super::MAX_SUBAGENT_REPORT_CHARACTERS)));
        assert!(bounded.ends_with("report cut at 20000 characters"));
    }
}
