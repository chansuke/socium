//! `-p`: one turn without the terminal UI. Standard output carries only
//! JSON Lines, one record per line, each with the schema version in `v`:
//!
//! - `started`: the session ID and the model;
//! - `activity`: an `ActivityEvent` as the Activity pane and the session's
//!   activity log record it;
//! - `finished`: `ok`, the reply text or the error, notices, and the tokens
//!   the turn used.
//!
//! Diagnostics go to standard error. No one can answer an approval here, so
//! a call that needs one is refused with a message that says how to allow
//! it. Ctrl+C, or standard output closing, cancels the turn.
use serde_json::{Value, json};
use socium_core::{AgentError, AgentProgress, AgentService, SessionStart};
use std::io::{Read, Write};

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Exit status: 0 when the turn finished, 1 when it failed, 2 for a usage
/// error, and 130 when it was cancelled.
pub(crate) async fn main(start: SessionStart, prompt: &str) -> i32 {
    let prompt = if prompt == "-" {
        let mut text = String::new();
        if let Err(err) = std::io::stdin().read_to_string(&mut text) {
            eprintln!("socium: cannot read the prompt from stdin: {err}");
            return 2;
        }
        text
    } else {
        prompt.to_string()
    };
    if prompt.trim().is_empty() {
        eprintln!("socium: the prompt is empty");
        return 2;
    }
    let mut agent = match AgentService::from_current_dir(start)
        .and_then(|mut agent| agent.wait_for_sandbox().map(|()| agent))
    {
        Ok(agent) => agent,
        Err(err) => {
            eprintln!("socium: {err}");
            return 1;
        }
    };
    let control = agent.task_control();
    let on_signal = control.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            on_signal.cancel();
        }
    });
    run(&mut agent, prompt, &mut std::io::stdout(), &mut |line| {
        eprintln!("socium: {line}")
    })
    .await
}

/// Runs `prompt` as one turn and writes its records to `out`. A write that
/// fails, such as a closed pipe, cancels the turn.
pub(crate) async fn run(
    agent: &mut AgentService,
    prompt: String,
    out: &mut (dyn Write + Send),
    diagnostic: &mut dyn FnMut(&str),
) -> i32 {
    for warning in agent.startup_warnings() {
        diagnostic(&warning);
    }
    let control = agent.task_control();
    let model = agent.active_model();
    let mut emit = |record: Value| {
        if writeln!(out, "{record}")
            .and_then(|()| out.flush())
            .is_err()
        {
            control.cancel();
        }
    };
    emit(record(
        "started",
        json!({"session": agent.session_id(), "model": model.alias, "model_id": model.model_id}),
    ));
    let result = agent
        .submit_with_progress(prompt, |event| {
            if let AgentProgress::Activity(activity) = event {
                emit(record("activity", json!({ "event": activity })));
            }
        })
        .await;
    let (usage, cost) = agent.session_usage();
    let (fields, status) = match &result {
        Ok(reply) => (
            json!({"ok": true, "text": reply.text, "notices": reply.notices}),
            0,
        ),
        Err(err) => (
            json!({"ok": false, "error": err.to_string()}),
            if matches!(err, AgentError::Cancelled) {
                130
            } else {
                1
            },
        ),
    };
    let mut finished = record("finished", fields);
    finished["session"] = json!(agent.session_id());
    finished["usage"] = json!(usage);
    finished["cost_usd"] = json!(cost);
    emit(finished);
    status
}

fn record(kind: &str, fields: Value) -> Value {
    let mut record = json!({ "v": SCHEMA_VERSION, "type": kind });
    if let (Some(record), Value::Object(fields)) = (record.as_object_mut(), fields) {
        record.extend(fields);
    }
    record
}
