//! `-p`: one turn without the terminal UI. Piped standard input comes
//! before the prompt; input that sends nothing for `STDIN_WAIT` is skipped
//! with a warning, since an agent's shell or a CI runner may hand over one
//! that never closes. What standard output carries depends on the format:
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
//! it, unless `--dangerously-skip-permissions` lets every call run. Ctrl+C, or standard output closing, cancels the turn.
use crate::cli::{OutputFormat, Print};
use serde_json::{Value, json};
use socium_core::{AgentError, AgentProgress, AgentService, SessionStart};
use std::io::{IsTerminal, Read, Write};
use std::time::Duration;

pub(crate) const SCHEMA_VERSION: u32 = 1;

/// How long standard input may stay silent before `-p` goes on without it.
/// Once its first byte or its end arrives, `-p` reads it to the end.
const STDIN_WAIT: Duration = Duration::from_secs(3);

/// Exit status: 0 when the turn finished, 1 when it failed, 2 for a usage
/// error, and 130 when it was cancelled.
#[expect(
    clippy::print_stderr,
    reason = "socium -p reports errors on stderr and has no terminal UI"
)]
pub(crate) async fn main(start: SessionStart, print: Print, skip_permissions: bool) -> i32 {
    let stdin = if std::io::stdin().is_terminal() {
        String::new()
    } else {
        match read_piped_stdin(STDIN_WAIT) {
            Ok(Some(text)) => text,
            Ok(None) => {
                eprintln!(
                    "socium: standard input sent nothing for {} seconds, so it is not read; \
                     redirect it from a file, or use < /dev/null when there is none",
                    STDIN_WAIT.as_secs()
                );
                String::new()
            }
            Err(err) => {
                eprintln!("socium: cannot read the prompt from stdin: {err}");
                return 2;
            }
        }
    };
    let prompt = join_prompt(&stdin, print.prompt.as_deref());
    if prompt.trim().is_empty() {
        eprintln!("socium: the prompt is empty");
        return 2;
    }
    let mut agent = match AgentService::from_current_dir(start)
        .and_then(|mut agent| agent.wait_for_sandbox().map(|()| agent))
    {
        Ok(mut agent) => {
            if skip_permissions {
                agent.skip_approvals();
            }
            agent
        }
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

/// All of standard input, or `None` when neither a byte nor its end arrives
/// within `wait`. The reading thread is left blocked then; the process exits
/// without it.
fn read_piped_stdin(wait: Duration) -> std::io::Result<Option<String>> {
    let (started, first_read) = std::sync::mpsc::channel();
    let (sender, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut bytes = Vec::new();
        let mut chunk = [0; 8192];
        let first = loop {
            match stdin.read(&mut chunk) {
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                other => break other,
            }
        };
        let _ = started.send(());
        let result = first.and_then(|read| {
            if read > 0 {
                bytes.extend_from_slice(&chunk[..read]);
                stdin.read_to_end(&mut bytes)?;
            }
            String::from_utf8(bytes).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "stdin is not UTF-8")
            })
        });
        let _ = sender.send(result);
    });
    if first_read.recv_timeout(wait).is_err() {
        return Ok(None);
    }
    finished
        .recv()
        .map_err(|_| std::io::Error::other("the stdin reader stopped"))?
        .map(Some)
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
                emit(&printable(&reply.text));
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

/// `text` without the control characters a terminal acts on, such as escape
/// sequences that set the title or write to the clipboard. Tabs and line
/// breaks stay; `\r\n` becomes `\n`.
fn printable(text: &str) -> String {
    text.chars()
        .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
        .collect()
}

fn record(kind: &str, fields: Value) -> Value {
    let mut record = json!({ "v": SCHEMA_VERSION, "type": kind });
    if let (Some(record), Value::Object(fields)) = (record.as_object_mut(), fields) {
        record.extend(fields);
    }
    record
}
