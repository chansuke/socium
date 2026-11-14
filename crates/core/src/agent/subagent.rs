//! The `agent` tool: runs a delegated task in a fresh context with its own
//! tool loop and returns only the subagent's final report as the tool result.
//!
//! A subagent shares the main agent's model, workspace, cancellation, and
//! approvals. It never sees the main conversation, and its tools are a
//! subset chosen by its type that never includes `agent`, so subagents do
//! not nest.

use crate::AgentProgress;
use crate::agent::turn::{TurnContext, extract_text, run_agent_turn};
use crate::agent::types::Message;
use crate::constants::{
    AGENT_TOOL_NAME, MAX_SUBAGENT_REPORT_CHARACTERS, MAX_TOOL_ROUNDS,
    SUBAGENT_EXPLORER_SYSTEM_PROMPT, SUBAGENT_WORKER_SYSTEM_PROMPT,
};
use crate::error::AgentError;
use crate::tools::error::ToolError;
use crate::tools::input::{require_object_input, require_string_field};
use crate::tools::{CustomToolDefinition, ToolDefinition};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;

/// What a subagent may do, chosen by the delegating model per call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubagentKind {
    /// Reads and searches only.
    Explorer,
    /// Every tool except `agent`; commands and commits still need approval.
    Worker,
}

impl SubagentKind {
    fn parse(value: &str) -> Result<Self, ToolError> {
        match value {
            "explorer" => Ok(Self::Explorer),
            "worker" => Ok(Self::Worker),
            other => Err(ToolError::Message(format!(
                "Unknown agent_type '{other}'; use 'explorer' or 'worker'"
            ))),
        }
    }

    fn system_prompt(self) -> &'static str {
        match self {
            Self::Explorer => SUBAGENT_EXPLORER_SYSTEM_PROMPT,
            Self::Worker => SUBAGENT_WORKER_SYSTEM_PROMPT,
        }
    }

    pub(crate) fn allows(self, tool: &str) -> bool {
        match self {
            Self::Explorer => matches!(
                tool,
                "read_file"
                    | "list_files"
                    | "search_text"
                    | "find_symbol"
                    | "git_branch"
                    | "git_status"
                    | "git_diff"
                    | "git_log"
            ),
            Self::Worker => tool != AGENT_TOOL_NAME,
        }
    }
}

pub(crate) fn agent_tool_definition() -> ToolDefinition {
    ToolDefinition::Custom(CustomToolDefinition {
        name: AGENT_TOOL_NAME.to_string(),
        description: "Delegate a self-contained task to a subagent. The subagent starts with an empty context, works with its own tools until it is done, and returns one written report as this tool's result; nothing else it saw comes back. Use it to keep a broad search or a separable piece of work out of this conversation, not for a single file read or search. The subagent cannot see this conversation, so put everything it needs in prompt: the goal, relevant paths and constraints, and what the report must contain. agent_type 'explorer' can only read, list, and search files and inspect Git; 'worker' can also edit files and run commands, which still need the user's approval. Subagents cannot start subagents. Read the report critically and tell the user what matters; they do not see it.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short label for the task, 3 to 6 words, shown to the user."
                },
                "prompt": {
                    "type": "string",
                    "description": "The complete task for the subagent, with the context it needs and what to report."
                },
                "agent_type": {
                    "type": "string",
                    "enum": ["explorer", "worker"],
                    "description": "'explorer' for read-only investigation; 'worker' when the task needs edits or commands."
                }
            },
            "required": ["description", "prompt", "agent_type"],
            "additionalProperties": false
        }),
    })
}

type ReportFuture<'a> = Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>>;

/// Runs the subagent of one `agent` call to the end and returns its report.
///
/// Its activity is forwarded tagged with `call_id`; its streamed text is
/// not, since it reaches the main agent only as the report. Its messages
/// are not saved: a restart during the call leaves the call's outcome
/// unknown, like any other interrupted tool.
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
        require_string_field(fields, "description", AGENT_TOOL_NAME)?;
        let prompt = require_string_field(fields, "prompt", AGENT_TOOL_NAME)?;
        let kind =
            SubagentKind::parse(require_string_field(fields, "agent_type", AGENT_TOOL_NAME)?)?;
        if prompt.trim().is_empty() {
            return Err(ToolError::Message(
                "agent requires a non-empty prompt".into(),
            ));
        }

        let tools = ctx
            .tools
            .iter()
            .filter(|tool| kind.allows(tool.name()))
            .cloned()
            .collect::<Vec<_>>();
        let child = TurnContext {
            system: kind.system_prompt(),
            tools: &tools,
            ..*ctx
        };
        let mut messages = vec![Message::user_text(prompt.to_string())];
        let agent = Some(call_id.to_string());
        let mut forward = |event: AgentProgress| match event {
            AgentProgress::Activity(mut activity) => {
                activity.agent.clone_from(&agent);
                on_progress(AgentProgress::Activity(activity));
            }
            // These would read as the main agent's own stream.
            AgentProgress::AssistantText { .. }
            | AgentProgress::ThinkingText { .. }
            | AgentProgress::RawProviderEvent { .. }
            | AgentProgress::ModelRequestStarted { .. } => {}
            other => on_progress(other),
        };
        let result = run_agent_turn(&child, &mut messages, &mut forward, &mut |_, _| Ok(())).await;

        let notes = last_assistant_text(&messages);
        let side_effects = match kind {
            SubagentKind::Explorer => "",
            SubagentKind::Worker => {
                " Changes it made before stopping are kept; inspect the workspace before retrying."
            }
        };
        match result {
            Ok(_) if notes.trim().is_empty() => {
                Ok("The subagent finished without a report.".into())
            }
            Ok(_) => Ok(bounded_report(notes)),
            Err(AgentError::ToolRoundsExhausted) => Err(ToolError::Message(format!(
                "The subagent stopped after {MAX_TOOL_ROUNDS} rounds without finishing.{side_effects}{}",
                last_notes(&notes)
            ))),
            Err(AgentError::Cancelled) => Err(ToolError::Message(format!(
                "The subagent was cancelled.{side_effects}"
            ))),
            Err(err) => Err(ToolError::Message(format!(
                "The subagent failed: {err}.{side_effects}{}",
                last_notes(&notes)
            ))),
        }
    })
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
    use crate::providers::ProviderResponse;
    use crate::providers::error::ProviderError;
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
                text("a.txt:1 says hello"),
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
        assert_eq!(
            last_results(main),
            [
                json!({"type": "tool_result", "tool_use_id": "agent-1", "content": "a.txt:1 says hello"})
            ]
        );
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
            "Unknown agent_type 'admin'; use 'explorer' or 'worker'"
        );
        assert_eq!(results[1]["content"], "agent requires a non-empty prompt");
    }

    #[test]
    fn long_reports_are_cut_on_a_character_boundary() {
        let report = "é".repeat(super::MAX_SUBAGENT_REPORT_CHARACTERS + 5);

        let bounded = super::bounded_report(report);

        assert!(bounded.starts_with(&"é".repeat(super::MAX_SUBAGENT_REPORT_CHARACTERS)));
        assert!(bounded.ends_with("report cut at 20000 characters"));
    }
}
