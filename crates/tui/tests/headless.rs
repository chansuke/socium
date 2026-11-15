//! `socium-tui -p` end to end: the real binary against a local server that
//! speaks the Anthropic streaming format. The environment is cleared, so no
//! provider key or user configuration is read, and nothing leaves the host.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

/// What the fake server does with each request, in order.
enum Reply {
    Events(Vec<Value>),
    /// Accept the request and never answer.
    Stall,
}

fn text_reply(text: &str) -> Reply {
    Reply::Events(vec![
        json!({"type": "message_start", "message": {"usage": {"input_tokens": 12}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 3}}),
        json!({"type": "message_stop"}),
    ])
}

fn tool_reply(name: &str, input: Value) -> Reply {
    Reply::Events(vec![
        json!({"type": "message_start", "message": {"usage": {"input_tokens": 12}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "call-1", "name": name, "input": {}}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": input.to_string()}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 5}}),
        json!({"type": "message_stop"}),
    ])
}

/// Serves `replies` on a loopback port and records each request body.
fn serve(replies: Vec<Reply>) -> (u16, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let recorded = bodies.clone();
    thread::spawn(move || {
        let mut held = Vec::new();
        for reply in replies {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let body = read_request(&stream);
            recorded.lock().unwrap().push(body);
            match reply {
                Reply::Stall => held.push(stream),
                Reply::Events(events) => {
                    let mut stream = stream;
                    let mut response = String::from(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                    );
                    for event in events {
                        response.push_str(&format!("data: {event}\n\n"));
                    }
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        }
        // Keep stalled connections open until the client gives up.
        thread::sleep(std::time::Duration::from_secs(30));
        drop(held);
    });
    (port, bodies)
}

fn read_request(stream: &TcpStream) -> Value {
    let mut reader = BufReader::new(stream);
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

struct Fixture {
    root: tempfile::TempDir,
    workspace: std::path::PathBuf,
}

fn fixture(port: u16) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(root.path().join("home")).unwrap();
    std::fs::write(
        root.path().join("models.toml"),
        format!(
            "default = \"local\"\n[providers.local]\nwire = \"anthropic\"\nbase_url = \"http://127.0.0.1:{port}\"\napi_key_env = \"LOCAL_KEY\"\n[[models]]\nalias = \"local\"\nprovider = \"local\"\nid = \"local-model\"\n"
        ),
    )
    .unwrap();
    Fixture { root, workspace }
}

fn command(fixture: &Fixture, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_socium-tui"));
    command
        .args(args)
        .current_dir(&fixture.workspace)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", fixture.root.path().join("home"))
        .env("LOCAL_KEY", "test-key")
        .env(
            "SOCIUM_MODELS_FILE",
            fixture.root.path().join("models.toml"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The HTTP client needs CA certificates even for a loopback server; a
    // build sandbox has them only through these variables.
    for name in ["SSL_CERT_FILE", "NIX_SSL_CERT_FILE"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

/// Every line of stdout as JSON; the test fails on anything else.
fn records(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
}

fn of_type<'a>(records: &'a [Value], kind: &str) -> Vec<&'a Value> {
    records.iter().filter(|r| r["type"] == kind).collect()
}

#[test]
fn a_turn_prints_only_versioned_json_lines_and_its_result() {
    let (port, bodies) = serve(vec![text_reply("hello from the fake model")]);
    let fixture = fixture(port);
    let output = command(&fixture, &["-p", "say hello"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let records = records(&output);
    assert!(records.iter().all(|r| r["v"] == 1), "{records:#?}");
    assert_eq!(records[0]["type"], "started");
    assert_eq!(records[0]["model"], "local");
    assert!(!of_type(&records, "activity").is_empty());
    let finished = records.last().unwrap();
    assert_eq!(finished["type"], "finished");
    assert_eq!(finished["ok"], true);
    assert_eq!(finished["text"], "hello from the fake model");
    assert_eq!(finished["usage"]["input_tokens"], 12);
    assert_eq!(finished["session"], records[0]["session"]);
    assert_eq!(
        bodies.lock().unwrap()[0]["messages"][0]["content"],
        "say hello"
    );
    assert!(!stderr.contains("\"v\""), "{stderr}");
}

#[test]
fn a_call_that_needs_approval_is_refused_and_the_model_is_told_why() {
    let (port, bodies) = serve(vec![
        tool_reply("run_command", json!({"command": ["touch", "created"]})),
        text_reply("understood"),
    ]);
    let fixture = fixture(port);
    let output = command(&fixture, &["-p", "make a file"]).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!fixture.workspace.join("created").exists());
    let bodies = bodies.lock().unwrap();
    let result = bodies[1]["messages"][2]["content"][0].clone();
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["is_error"], true);
    assert!(
        result["content"].as_str().unwrap().contains("config.toml"),
        "{result}"
    );
    let records = records(&output);
    assert!(
        of_type(&records, "activity")
            .iter()
            .any(|r| r["event"]["type"] == "tool_finished" && r["event"]["is_error"] == true),
        "{records:#?}"
    );
}

#[test]
fn an_interrupt_cancels_the_turn_and_still_reports_it() {
    let (port, _) = serve(vec![Reply::Stall]);
    let fixture = fixture(port);
    let mut child = command(&fixture, &["-p", "wait"]).spawn().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    stdout.read_line(&mut first).unwrap();
    assert!(first.contains("\"started\""), "{first}");
    thread::sleep(std::time::Duration::from_millis(500));
    let killed = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130));
    let finished: Value = serde_json::from_str(rest.lines().last().unwrap()).unwrap();
    assert_eq!(finished["type"], "finished");
    assert_eq!(finished["ok"], false);
    assert!(
        finished["error"].as_str().unwrap().contains("cancelled"),
        "{finished}"
    );
}
