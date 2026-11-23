//! Private JSON Lines transport for the local Socium client gateway.
//! The gateway handles authentication. Approval and execution stay in core.

use serde::Deserialize;
use serde_json::{Value, json};
use socium_core::plan::PlanView;
use socium_core::{
    ActivityKind, AgentError, AgentProgress, AgentService, ApprovalAnswer, SessionStart,
    TranscriptEntry, TranscriptRole,
};
use std::io::{self, BufRead, Write};
use tokio::sync::mpsc;

#[derive(Deserialize)]
struct Request {
    id: String,
    #[serde(flatten)]
    command: Command,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Command {
    New,
    Resume {
        session: String,
    },
    Prompt {
        text: String,
    },
    Cancel,
    Approve {
        request: u64,
        allow: bool,
        #[serde(default)]
        scope: ApprovalScope,
    },
    SetModel {
        model: String,
    },
}

/// How far an allow reaches; a denial ignores it.
#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ApprovalScope {
    #[default]
    Once,
    /// What the request offered as its session option, until Socium exits.
    Session,
}

impl ApprovalScope {
    fn answer(self, allow: bool) -> ApprovalAnswer {
        match (allow, self) {
            (false, _) => ApprovalAnswer::Deny,
            (true, Self::Once) => ApprovalAnswer::AllowOnce,
            (true, Self::Session) => ApprovalAnswer::AllowForSession,
        }
    }
}

#[expect(
    clippy::print_stderr,
    reason = "stdout carries the protocol, and the gateway passes stderr through to its own"
)]
pub(crate) async fn run() -> color_eyre::Result<()> {
    let mut agent = AgentService::from_current_dir(SessionStart::Continue)?;
    agent.wait_for_sandbox()?;
    agent.task_control().enable_interactive_approval();
    let (send, mut receive) = mpsc::channel(16);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<Request>(&line) {
                Ok(request) => {
                    if send.blocking_send(request).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    eprintln!("socium client request: {error}");
                    break;
                }
            }
        }
    });
    publish_session(&mut agent)?;
    while let Some(Request { id, command }) = receive.recv().await {
        match command {
            Command::Prompt { text } => {
                emit(json!({"type": "running", "turn": id, "text": text}));
                respond(&id, Ok(()));
                agent = run_turn(agent, text, &mut receive).await?;
            }
            command => {
                let result = match command {
                    Command::New => agent.clear().map_err(AgentError::from),
                    Command::Resume { session } => agent.resume_session(&session).map(|_| ()),
                    Command::SetModel { model } => agent.switch_model(&model).map(|switch| {
                        // The gateway shows these with the session that follows.
                        if !switch.warnings.is_empty() {
                            emit(json!({"type": "outcome", "notices": switch.warnings}));
                        }
                    }),
                    _ => Err(io::Error::other("There is no active turn.").into()),
                };
                if result.is_ok() {
                    publish_session(&mut agent)?;
                }
                respond(&id, result);
            }
        }
    }
    Ok(())
}

async fn run_turn(
    mut agent: AgentService,
    text: String,
    receive: &mut mpsc::Receiver<Request>,
) -> color_eyre::Result<AgentService> {
    let control = agent.task_control();
    control.reset_cancellation();
    let mut task = tokio::spawn(async move {
        let mut publisher = ProgressPublisher::default();
        let result = agent
            .submit_with_progress(text, |progress| publisher.publish(progress))
            .await;
        (agent, result, publisher.warnings)
    });
    let (mut agent, result, warnings) = loop {
        tokio::select! {
            result = &mut task => break result?,
            request = receive.recv() => {
                let Some(Request { id, command }) = request else {
                    control.cancel();
                    break task.await?;
                };
                let result = match command {
                    Command::Cancel => {
                        control.cancel();
                        Ok(())
                    }
                    Command::Approve {
                        request,
                        allow,
                        scope,
                    } if !control.is_cancelled() => {
                        if control.answer(request, scope.answer(allow)) {
                            Ok(())
                        } else {
                            Err(io::Error::other("This approval request has already closed.").into())
                        }
                    }
                    _ => Err(io::Error::other("Finish or stop the current turn first.").into()),
                };
                respond(&id, result);
            }
        }
    };

    let outcome = match result {
        Ok(reply) => reply.notices,
        Err(AgentError::Cancelled) => vec!["Turn stopped.".to_string()],
        Err(error) => vec![error.to_string()],
    };
    emit(json!({"type": "outcome", "notices": turn_notices(warnings, outcome)}));
    publish_session(&mut agent)?;
    Ok(agent)
}

fn publish_session(agent: &mut AgentService) -> io::Result<()> {
    let sessions = agent
        .list_sessions()?
        .into_iter()
        .map(|session| {
            json!({
                "id": session.id,
                "title": session.name.filter(|name| !name.is_empty()).unwrap_or(session.preview),
                "readable": session.readable,
            })
        })
        .collect::<Vec<_>>();
    let models = agent
        .available_models()
        .into_iter()
        .map(|model| {
            json!({
                "alias": model.alias,
                "provider": model.provider,
                "description": model.description,
                "unavailable": model.unavailable,
            })
        })
        .collect::<Vec<_>>();
    let messages = agent
        .transcript_entries()
        .into_iter()
        .map(entry)
        .collect::<Vec<_>>();
    emit(json!({
        "type": "session",
        "session": agent.session_id(),
        "title": agent.session_name(),
        "model": agent.active_model().alias,
        "models": models,
        "plan": agent.plan().map(compact_plan),
        "sessions": sessions,
        "messages": messages,
        "warnings": agent.startup_warnings(),
    }));
    Ok(())
}

fn entry(entry: TranscriptEntry) -> Value {
    let role = match entry.role {
        TranscriptRole::User => "user",
        TranscriptRole::Assistant => "assistant",
        TranscriptRole::System => "system",
        TranscriptRole::Tool { failed: false } => "tool",
        TranscriptRole::Tool { failed: true } => "tool_error",
    };
    json!({"role": role, "text": entry.text})
}

/// The plan without per-task history, which clients do not show.
fn compact_plan(plan: PlanView) -> Value {
    let tasks = plan
        .rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "depth": row.depth,
                "title": row.title,
                "status": row.status,
                "group": row.is_group,
                "note": row.note,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "goal": plan.goal,
        "revision": plan.revision,
        "done": plan.progress.done,
        "tasks": tasks,
    })
}

/// Longest activity detail sent to clients, in characters.
const MAX_ACTIVITY_DETAIL: usize = 120;

/// Forwards progress as client events. Activity is one line of what the
/// agent does now, sent only when it changes.
#[derive(Default)]
struct ProgressPublisher {
    activity: Option<(&'static str, Option<String>)>,
    /// Warnings of the turn, such as an MCP server that did not start or a
    /// renewed token that could not be saved, which end up in its notices.
    warnings: Vec<String>,
}

impl ProgressPublisher {
    fn publish(&mut self, progress: AgentProgress) {
        let event = match progress {
            AgentProgress::AssistantText { text } => {
                self.activity("responding", None);
                json!({"type": "delta", "text": text})
            }
            AgentProgress::ToolCallRecorded(tool) => {
                json!({"type": "tool", "message": entry(tool)})
            }
            AgentProgress::ApprovalRequested {
                request_id,
                tool,
                summary,
                input,
                requested_by,
                session_option,
            } => {
                self.activity("approval", Some(&tool));
                json!({
                    "type": "approval",
                    "request": request_id,
                    "tool": tool,
                    "summary": summary,
                    "input": input,
                    "requestedBy": requested_by,
                    "sessionOption": session_option,
                })
            }
            AgentProgress::ApprovalResolved { request_id, .. } => {
                // The call goes on (or reports its refusal) after the answer.
                if let Some(("approval", tool)) = self.activity.clone() {
                    self.activity("tool", tool.as_deref());
                }
                json!({"type": "approval_resolved", "request": request_id})
            }
            AgentProgress::Activity(event) => match event.kind {
                ActivityKind::PlanUpdated { plan } => {
                    json!({"type": "plan", "plan": compact_plan(plan)})
                }
                ActivityKind::Warning { message } => {
                    self.warnings.push(message);
                    return;
                }
                _ => return,
            },
            AgentProgress::ModelRequestStarted { provider, .. } => {
                return self.activity("requesting", Some(&provider));
            }
            AgentProgress::ThinkingText { .. } => return self.activity("thinking", None),
            AgentProgress::ToolCallStarted { name } => return self.activity("tool", Some(&name)),
            AgentProgress::CommandStarted { program, .. } => {
                let program = (!program.is_empty()).then_some(program.as_str());
                return self.activity("command", program);
            }
            AgentProgress::CompactionStarted => return self.activity("compacting", None),
            _ => return,
        };
        emit(event);
    }

    fn activity(&mut self, phase: &'static str, detail: Option<&str>) {
        let detail = detail.map(|text| text.chars().take(MAX_ACTIVITY_DETAIL).collect::<String>());
        let next = Some((phase, detail));
        if self.activity == next {
            return;
        }
        self.activity = next;
        if let Some((phase, detail)) = &self.activity {
            emit(json!({"type": "activity", "phase": phase, "detail": detail}));
        }
    }
}

/// The turn's warnings, each once, then what its outcome says.
fn turn_notices(warnings: Vec<String>, outcome: Vec<String>) -> Vec<String> {
    let mut notices = Vec::new();
    for notice in warnings.into_iter().chain(outcome) {
        if !notices.contains(&notice) {
            notices.push(notice);
        }
    }
    notices
}

fn respond(id: &str, result: Result<(), AgentError>) {
    emit(
        json!({"type": "response", "id": id, "error": result.err().map(|error| error.to_string())}),
    );
}

fn emit(value: Value) {
    let _ = writeln!(io::stdout().lock(), "{value}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use socium_core::ActivityEvent;

    fn activity(kind: ActivityKind) -> AgentProgress {
        AgentProgress::Activity(ActivityEvent {
            turn_id: 1,
            round: 0,
            at_ms: 0,
            agent: None,
            kind,
        })
    }

    #[test]
    fn warnings_of_a_turn_reach_its_notices() {
        let text = "MCP server github needs you to log in: run socium mcp login github";
        let mut publisher = ProgressPublisher::default();
        publisher.publish(activity(ActivityKind::Warning {
            message: text.into(),
        }));
        publisher.publish(activity(ActivityKind::TurnStarted));
        assert_eq!(publisher.warnings, [text]);
        assert_eq!(
            turn_notices(
                vec![text.into(), "saved token".into(), text.into()],
                vec!["Turn stopped.".into()]
            ),
            [text, "saved token", "Turn stopped."]
        );
    }

    #[test]
    fn an_approval_answer_reaches_as_far_as_its_scope() {
        let answer = |line: &str| {
            let request: Request = serde_json::from_str(line).unwrap();
            let Command::Approve { allow, scope, .. } = request.command else {
                panic!("an approval answer");
            };
            scope.answer(allow)
        };
        assert_eq!(
            answer(
                r#"{"id": "a", "action": "approve", "request": 7, "allow": true, "scope": "session"}"#
            ),
            ApprovalAnswer::AllowForSession
        );
        // Older clients send no scope.
        assert_eq!(
            answer(r#"{"id": "a", "action": "approve", "request": 7, "allow": true}"#),
            ApprovalAnswer::AllowOnce
        );
        assert_eq!(
            answer(
                r#"{"id": "a", "action": "approve", "request": 7, "allow": false, "scope": "session"}"#
            ),
            ApprovalAnswer::Deny
        );
    }
}
