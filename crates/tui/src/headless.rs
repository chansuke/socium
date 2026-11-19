//! `-p`: one turn without the terminal UI. Piped standard input comes
//! before the prompt. What standard output carries depends on the format:
//!
//! - `text`, the default: the reply text alone; an error, notices, and
//!   diagnostics go to standard error;
//! - `stream-json`: JSON Lines, one record per line, each with the schema
//!   version in `v`:
//!   - `started`: the session ID and the model;
//!   - `activity`: an `ActivityEvent` as the Activity pane and the session's
//!     activity log record it;
//!   - `finished`: `ok`, the reply text or the error, notices, the plan, and the tokens
//!     the turn used;
//! - `json`: the `finished` record alone.
//!
//! Diagnostics go to standard error. No one can answer an approval here, so
//! a call that needs one is refused with a message that says how to allow
//! it. Ctrl+C, or standard output closing, cancels the turn.
use crate::cli::{OutputFormat, Print};
use serde_json::{Value, json};
use socium_core::{AgentError, AgentProgress, AgentService, SessionStart};
use std::io::{IsTerminal, Read, Write};

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Exit status: 0 when the turn finished, 1 when it failed, 2 for a usage
/// error, and 130 when it was cancelled.
pub(crate) async fn main(start: SessionStart, print: Print) -> i32 {
    let mut stdin = String::new();
    if !std::io::stdin().is_terminal()
        && let Err(err) = std::io::stdin().read_to_string(&mut stdin)
    {
        eprintln!("socium: cannot read the prompt from stdin: {err}");
        return 2;
    }
    let prompt = join_prompt(&stdin, print.prompt.as_deref());
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
    run(
        &mut agent,
        prompt,
        print.format,
        &mut std::io::stdout(),
        &mut |line| eprintln!("socium: {line}"),
    )
    .await
}

/// Piped input, a blank line, then the prompt; either alone when the other
/// is missing. Line endings that close the piped input are dropped.
fn join_prompt(stdin: &str, prompt: Option<&str>) -> String {
    let stdin = stdin.trim_end_matches(['\r', '\n']);
    match prompt {
        Some(prompt) if !stdin.is_empty() => format!("{stdin}\n\n{prompt}"),
        Some(prompt) => prompt.to_string(),
        None => stdin.to_string(),
    }
}

/// Runs `prompt` as one turn and writes what `format` asks for to `out`. A
/// write that fails, such as a closed pipe, cancels the turn.
pub(crate) async fn run(
    agent: &mut AgentService,
    prompt: String,
    format: OutputFormat,
    out: &mut (dyn Write + Send),
    diagnostic: &mut dyn FnMut(&str),
) -> i32 {
    for warning in agent.startup_warnings() {
        diagnostic(&warning);
    }
    let control = agent.task_control();
    let model = agent.active_model();
    let mut emit = |line: &dyn std::fmt::Display| {
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            control.cancel();
        }
    };
    let stream = format == OutputFormat::StreamJson;
    if stream {
        emit(&record(
            "started",
            json!({"session": agent.session_id(), "model": model.alias, "model_id": model.model_id}),
        ));
    }
    let result = agent
        .submit_with_progress(prompt, |event| {
            if stream && let AgentProgress::Activity(activity) = event {
                emit(&record("activity", json!({ "event": activity })));
            }
        })
        .await;
    let status = match &result {
        Ok(_) => 0,
        Err(AgentError::Cancelled) => 130,
        Err(_) => 1,
    };
    if format == OutputFormat::Text {
        match &result {
            Ok(reply) => {
                for notice in &reply.notices {
                    diagnostic(notice);
                }
                emit(&reply.text);
            }
            Err(err) => diagnostic(&err.to_string()),
        }
        return status;
    }
    let (usage, cost) = agent.session_usage();
    let unreported_calls = agent.unreported_model_calls();
    let fields = match &result {
        Ok(reply) => json!({"ok": true, "text": reply.text, "notices": reply.notices}),
        Err(err) => json!({"ok": false, "error": err.to_string()}),
    };
    let mut finished = record("finished", fields);
    finished["session"] = json!(agent.session_id());
    finished["usage"] = json!(usage);
    finished["cost_usd"] = json!(cost);
    // Calls that reported no usage: `usage` and `cost_usd` are then lower bounds.
    finished["unreported_calls"] = json!(unreported_calls);
    // The plan the session's `update_plan` calls left; null without one.
    finished["plan"] = json!(agent.plan());
    emit(&finished);
    status
}

fn record(kind: &str, fields: Value) -> Value {
    let mut record = json!({ "v": SCHEMA_VERSION, "type": kind });
    if let (Some(record), Value::Object(fields)) = (record.as_object_mut(), fields) {
        record.extend(fields);
    }
    record
}
