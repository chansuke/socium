//! Private JSON Lines transport for the local Socium client gateway.
//! The gateway handles authentication. Approval and execution stay in core.

use serde::Deserialize;
use serde_json::{Value, json};
use socium_core::{
    AgentError, AgentProgress, AgentService, SessionStart, TranscriptEntry, TranscriptRole,
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
    Resume { session: String },
    Prompt { text: String },
    Cancel,
    Approve { request: u64, allow: bool },
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
    let mut task = tokio::spawn(async move {
        let result = agent.submit_with_progress(text, publish_progress).await;
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
                    Command::Approve { request, allow } if !control.is_cancelled() => {
                        if control.answer_approval(request, allow) {
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

    match result {
        Ok(reply) => emit(json!({"type": "outcome", "notices": reply.notices})),
        Err(AgentError::Cancelled) => {
            emit(json!({"type": "outcome", "notices": ["Turn stopped."]}))
        }
        Err(error) => emit(json!({"type": "outcome", "notices": [error.to_string()]})),
    }
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

fn publish_progress(progress: AgentProgress) {
    let event = match progress {
        AgentProgress::AssistantText { text } => json!({"type": "delta", "text": text}),
        AgentProgress::ToolCallRecorded(tool) => json!({"type": "tool", "message": entry(tool)}),
        AgentProgress::ApprovalRequested {
            request_id,
            tool,
            summary,
            input,
            requested_by,
            ..
        } => json!({
            "type": "approval",
            "request": request_id,
            "tool": tool,
            "summary": summary,
            "input": input,
            "requestedBy": requested_by,
        }),
        AgentProgress::ApprovalResolved { request_id, .. } => {
            json!({"type": "approval_resolved", "request": request_id})
        }
        _ => return,
    };
    emit(event);
}

fn respond(id: &str, result: Result<(), AgentError>) {
    emit(
        json!({"type": "response", "id": id, "error": result.err().map(|error| error.to_string())}),
    );
}

fn emit(value: Value) {
    let _ = writeln!(io::stdout().lock(), "{value}");
}
