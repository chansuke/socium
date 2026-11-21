//! Private JSON Lines transport for the local Socium client gateway.
//! The gateway handles authentication. Approval and execution stay in core.

use serde::Deserialize;
use serde_json::{Value, json};
use socium_core::{
    ActivityEvent, ActivityKind, AgentError, AgentProgress, AgentService, ApprovalAnswer,
    SessionStart, TranscriptEntry, TranscriptRole,
};
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
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
    /// With `forSession`, an allow also allows what the request offered
    /// (`sessionOption`) until Socium exits.
    Approve {
        request: u64,
        allow: bool,
        #[serde(default, rename = "forSession")]
        for_session: bool,
    },
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
    // Warnings of the turn, such as an MCP server that did not start or a
    // renewed token that could not be saved, end up in its notices.
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let collected = warnings.clone();
    let mut task = tokio::spawn(async move {
        let result = agent
            .submit_with_progress(text, move |progress| match forward(progress) {
                Forward::Event(event) => emit(event),
                Forward::Warning(message) => collected
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .push(message),
                Forward::Nothing => {}
            })
            .await;
        (agent, result)
    });
    let (mut agent, result) = loop {
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
                    Command::Approve { request, allow, for_session } if !control.is_cancelled() => {
                        if control.answer(request, approval_answer(allow, for_session)) {
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
    let warnings = std::mem::take(&mut *warnings.lock().unwrap_or_else(|err| err.into_inner()));
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

/// What one progress event becomes for the gateway.
enum Forward {
    Event(Value),
    /// A warning, which goes into the turn's notices.
    Warning(String),
    Nothing,
}

fn forward(progress: AgentProgress) -> Forward {
    Forward::Event(match progress {
        AgentProgress::AssistantText { text } => json!({"type": "delta", "text": text}),
        AgentProgress::ToolCallRecorded(tool) => json!({"type": "tool", "message": entry(tool)}),
        AgentProgress::ApprovalRequested {
            request_id,
            tool,
            summary,
            input,
            requested_by,
            session_option,
        } => json!({
            "type": "approval",
            "request": request_id,
            "tool": tool,
            "summary": summary,
            "input": input,
            "requestedBy": requested_by,
            "sessionOption": session_option,
        }),
        AgentProgress::ApprovalResolved { request_id, .. } => {
            json!({"type": "approval_resolved", "request": request_id})
        }
        AgentProgress::Activity(ActivityEvent {
            kind: ActivityKind::Warning { message },
            ..
        }) => return Forward::Warning(message),
        _ => return Forward::Nothing,
    })
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

/// The answer to an approval request: allowing for the session only
/// with `allow`.
fn approval_answer(allow: bool, for_session: bool) -> ApprovalAnswer {
    match (allow, for_session) {
        (false, _) => ApprovalAnswer::Deny,
        (true, false) => ApprovalAnswer::AllowOnce,
        (true, true) => ApprovalAnswer::AllowForSession,
    }
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

    fn warning(message: &str) -> AgentProgress {
        AgentProgress::Activity(ActivityEvent {
            turn_id: 1,
            round: 0,
            at_ms: 0,
            agent: None,
            kind: ActivityKind::Warning {
                message: message.into(),
            },
        })
    }

    #[test]
    fn warnings_of_a_turn_reach_its_notices() {
        let text = "MCP server github needs you to log in: run socium mcp login github";
        let Forward::Warning(message) = forward(warning(text)) else {
            panic!("a warning is forwarded");
        };
        assert_eq!(message, text);
        assert!(matches!(
            forward(AgentProgress::Activity(ActivityEvent {
                turn_id: 1,
                round: 0,
                at_ms: 0,
                agent: None,
                kind: ActivityKind::TurnStarted,
            })),
            Forward::Nothing
        ));
        assert_eq!(
            turn_notices(
                vec![text.into(), "saved token".into(), text.into()],
                vec!["Turn stopped.".into()]
            ),
            [text, "saved token", "Turn stopped."]
        );
    }

    #[test]
    fn an_approval_offers_and_takes_the_session_option() {
        let Forward::Event(event) = forward(AgentProgress::ApprovalRequested {
            request_id: 7,
            tool: "mcp__github__create_issue".into(),
            summary: String::new(),
            input: "Details".into(),
            requested_by: None,
            session_option: Some("`create_issue` of github".into()),
        }) else {
            panic!("an approval is an event");
        };
        assert_eq!(event["sessionOption"], "`create_issue` of github");
        assert_eq!(event["request"], 7);

        let request: Request = serde_json::from_str(
            r#"{"id": "a", "action": "approve", "request": 7, "allow": true, "forSession": true}"#,
        )
        .unwrap();
        let Command::Approve {
            request,
            allow,
            for_session,
        } = request.command
        else {
            panic!("an approval answer");
        };
        assert_eq!(
            (request, approval_answer(allow, for_session)),
            (7, ApprovalAnswer::AllowForSession)
        );
        // Older clients send no `forSession`.
        let request: Request = serde_json::from_str(
            r#"{"id": "a", "action": "approve", "request": 7, "allow": true}"#,
        )
        .unwrap();
        assert!(matches!(
            request.command,
            Command::Approve {
                for_session: false,
                ..
            }
        ));
        assert_eq!(approval_answer(false, true), ApprovalAnswer::Deny);
        assert_eq!(approval_answer(true, false), ApprovalAnswer::AllowOnce);
    }
}
