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
use crate::activity::{ActivityKind, summarize_tool_input};
use crate::agent::subagent::record::{SubagentRecord, SubagentStatus};
use crate::agent::turn::repair_incomplete_tools;
use crate::agent::turn::{TurnContext, extract_text, run_agent_turn};
use crate::agent::types::{Message, strip_thinking};
use crate::constants::{AGENT_TOOL_NAME, MAX_SUBAGENT_REPORT_CHARACTERS, MAX_TOOL_ROUNDS};
use crate::error::AgentError;
use crate::providers::TokenUsage;
use crate::tools::error::ToolError;
use crate::tools::input::{optional_string_field, require_object_input, require_string_field};
use crate::tools::tools_for_model;
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
        if prompt.trim().is_empty() {
            return Err(ToolError::Message(
                "agent requires a non-empty prompt".into(),
            ));
        }
        // The call's model, else the type's, else the current one.
        let alias =
            optional_string_field(fields, "model", AGENT_TOOL_NAME)?.or(kind.model.as_deref());
        let model = match alias {
            None => None,
            Some(alias) => Some(ctx.subagents.model(alias).ok_or_else(|| {
                ToolError::Message(format!(
                    "Model '{alias}' is not available to subagents; use one of: {}",
                    ctx.subagents.model_aliases().join(", ")
                ))
            })?),
        };

        // The type's tools, as its model is offered them.
        let model_tools;
        let child = match model {
            None => *ctx,
            Some(model) => {
                model_tools = tools_for_model(model.config.features.anthropic_text_editor);
                TurnContext {
                    provider: model.provider.as_ref(),
                    model: &model.config.id,
                    context_window: model.config.context_window,
                    max_tokens: model.max_tokens,
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
        let child = TurnContext {
            system: &kind.system_prompt,
            tools: &tools,
            ..child
        };
        let root = ctx.workspace_root;
        let state_error =
            |err: std::io::Error| ToolError::Message(format!("Subagent record: {err}"));
        let mut record = match optional_string_field(fields, "resume", AGENT_TOOL_NAME)? {
            Some(id) => {
                let mut record = record::load(root, id)
                    .map_err(state_error)?
                    .ok_or_else(|| {
                        ToolError::Message(format!("No saved subagent '{id}' to resume"))
                    })?;
                if record.agent_type != kind.name {
                    return Err(ToolError::Message(format!(
                        "Subagent '{id}' has agent_type '{}'; resume it with that type",
                        record.agent_type
                    )));
                }
                // A crash may have cut its last tool batch short, and its
                // thinking only goes back to the model that produced it.
                repair_incomplete_tools(&mut record.messages);
                if record.model != child.model {
                    strip_thinking(&mut record.messages);
                }
                record
            }
            None => {
                let id = record::new_id(root, call_id).map_err(state_error)?;
                SubagentRecord::new(id, &kind.name, description, child.model)
            }
        };
        let resumed_from = record.messages.len();
        let mut messages = std::mem::take(&mut record.messages);
        messages.push(Message::user_text(prompt.to_string()));
        record.messages.clone_from(&messages);
        record.model = child.model.to_string();
        record.runs += 1;
        record.status = SubagentStatus::Running;
        record.save(root).map_err(|err| {
            ToolError::Message(format!(
                "Could not save the subagent record; it was not started: {err}"
            ))
        })?;
        if record.runs == 1 {
            // Pruning only removes other, older records.
            let _ = record::prune(root);
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
                stats.observe(&activity.kind);
                activity.agent.clone_from(&agent);
                on_progress(AgentProgress::Activity(activity));
            }
            AgentProgress::ApprovalRequested {
                request_id,
                tool,
                summary,
                input,
                requested_by: _,
            } => on_progress(AgentProgress::ApprovalRequested {
                request_id,
                tool,
                summary,
                input,
                requested_by: Some(requested_by.clone()),
            }),
            // These would read as the main agent's own stream.
            AgentProgress::AssistantText { .. }
            | AgentProgress::ThinkingText { .. }
            | AgentProgress::RawProviderEvent { .. }
            | AgentProgress::ModelRequestStarted { .. } => {}
            other => on_progress(other),
        };
        // Saved before each side effect, like the main conversation.
        let mut checkpoint = |context: &[Message], _event: Value| -> std::io::Result<()> {
            record.messages = context.to_vec();
            record.save(root)
        };
        let result = run_agent_turn(&child, &mut messages, &mut forward, &mut checkpoint).await;
        record.messages = messages;
        record.status = match &result {
            Ok(_) => SubagentStatus::Finished,
            Err(AgentError::Cancelled) => SubagentStatus::Cancelled,
            Err(_) => SubagentStatus::Failed,
        };
        let saved = record.save(root);

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

/// What one subagent run cost, counted from its activity so that failed
/// and cancelled runs are measured too.
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

    fn observe(&mut self, kind: &ActivityKind) {
        match kind {
            ActivityKind::RoundStarted { .. } => self.rounds += 1,
            ActivityKind::ToolStarted { .. } => self.tool_calls += 1,
            ActivityKind::RoundFinished {
                usage: Some(usage), ..
            } => {
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
            _ => {}
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
    use crate::agent::types::{AgentMessageResponse, ContentBlock};
    use crate::constants::{
        DEFAULT_SYSTEM_PROMPT, SUBAGENT_EXPLORER_SYSTEM_PROMPT, SUBAGENT_WORKER_SYSTEM_PROMPT,
    };
    use crate::providers::error::ProviderError;
    use crate::providers::{ProviderResponse, TokenUsage};
    use crate::test_support::{ScriptedProvider, make_temp_dir, text_provider_response};
    use crate::{ActivityKind, AgentProgress, AgentService};
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

    fn scripted(
        root: &std::path::Path,
        responses: Vec<Result<ProviderResponse, ProviderError>>,
    ) -> (AgentService, Arc<Mutex<Vec<Value>>>) {
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
        assert_eq!(child["system"], SUBAGENT_EXPLORER_SYSTEM_PROMPT);
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
        expected.retain(|name| name != "agent");
        assert_eq!(requests[1]["system"], SUBAGENT_WORKER_SYSTEM_PROMPT);
        assert_eq!(tool_names(&requests[1]), expected);
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
                .starts_with("Sandbox command denied")
        );
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
                system_prompt: "You review.".into(),
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
        assert_eq!(child["system"], "You review.");
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
        assert_eq!(child["system"], SUBAGENT_EXPLORER_SYSTEM_PROMPT);
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

    fn record(root: &std::path::Path, id: &str) -> Value {
        let path = root.join(".socium/agents").join(format!("{id}.json"));
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
        let saved = record(workspace.path(), "agent-1");
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
        let saved = record(workspace.path(), "agent-1");
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
                "read_file a.txt\nPath: a.txt",
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
                "No saved subagent 'missing' to resume",
                "Subagent record: '../agent-1' is not a subagent ID",
                "Subagent 'agent-1' has agent_type 'explorer'; resume it with that type",
            ]
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

        let saved = record(workspace.path(), "agent-1");
        assert_eq!(saved["status"], "failed");
        let messages = saved["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "read-1");
        assert_eq!(requests.lock().unwrap().len(), 4);
        drop(service);

        // A crash between the call and its result leaves the call alone.
        let path = workspace.path().join(".socium/agents/agent-1.json");
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
        assert_eq!(record(workspace.path(), "agent-a")["status"], "finished");
        assert_eq!(record(workspace.path(), "agent-b")["status"], "finished");
    }

    #[test]
    fn long_reports_are_cut_on_a_character_boundary() {
        let report = "é".repeat(super::MAX_SUBAGENT_REPORT_CHARACTERS + 5);

        let bounded = super::bounded_report(report);

        assert!(bounded.starts_with(&"é".repeat(super::MAX_SUBAGENT_REPORT_CHARACTERS)));
        assert!(bounded.ends_with("report cut at 20000 characters"));
    }
}
