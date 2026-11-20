//! `socium -p` end to end: the real binary against a local server that
//! speaks the Anthropic streaming format. The environment is cleared, so no
//! provider key or user configuration is read, and nothing leaves the host.
#![expect(
    clippy::unwrap_used,
    reason = "fixture helpers outside #[test] functions fail the test by panicking"
)]
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_socium"));
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
    let output = command(
        &fixture,
        &["-p", "say hello", "--output-format", "stream-json"],
    )
    .output()
    .unwrap();
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
    let output = command(
        &fixture,
        &["-p", "make a file", "--output-format", "stream-json"],
    )
    .output()
    .unwrap();
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
fn skipping_permissions_runs_a_call_that_would_need_approval() {
    let (port, bodies) = serve(vec![
        tool_reply(
            "str_replace_based_edit_tool",
            json!({"command": "create", "path": "build.rs", "file_text": "fn main() {}\n"}),
        ),
        text_reply("done"),
    ]);
    let fixture = fixture(port);
    let output = command(
        &fixture,
        &["-p", "add a build script", "--dangerously-skip-permissions"],
    )
    .output()
    .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("--dangerously-skip-permissions"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("build.rs")).unwrap(),
        "fn main() {}\n"
    );
    let result = bodies.lock().unwrap()[1]["messages"][2]["content"][0].clone();
    assert_eq!(result["type"], "tool_result");
    assert_ne!(result["is_error"], true, "{result}");
}

#[test]
fn an_interrupt_cancels_the_turn_and_still_reports_it() {
    let (port, _) = serve(vec![Reply::Stall]);
    let fixture = fixture(port);
    let mut child = command(&fixture, &["--output-format", "stream-json", "-p", "wait"])
        .spawn()
        .unwrap();
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

#[test]
fn a_plan_the_model_records_reaches_the_activity_lines_and_the_result() {
    let (port, bodies) = serve(vec![
        tool_reply(
            "update_plan",
            json!({"goal": "Fix the parser", "tasks": [
                {"id": "repro", "title": "Reproduce", "status": "done"},
                {"id": "fix", "title": "Fix it", "status": "active"}
            ]}),
        ),
        text_reply("planned"),
    ]);
    let fixture = fixture(port);
    let output = command(
        &fixture,
        &["-p", "fix the parser", "--output-format", "stream-json"],
    )
    .output()
    .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bodies = bodies.lock().unwrap();
    assert!(
        bodies[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "update_plan")
    );
    let receipt = bodies[1]["messages"][2]["content"][0]["content"]
        .as_str()
        .unwrap();
    assert!(
        receipt.starts_with("Plan r1 \"Fix the parser\": 50% (1 of 2 tasks done), 1 active"),
        "{receipt}"
    );
    let records = records(&output);
    let updated = of_type(&records, "activity")
        .into_iter()
        .find(|r| r["event"]["type"] == "plan_updated")
        .unwrap_or_else(|| panic!("{records:#?}"));
    assert_eq!(updated["event"]["plan"]["goal"], "Fix the parser");
    let finished = records.last().unwrap();
    assert_eq!(finished["plan"], updated["event"]["plan"]);
}

/// Runs the binary with `stdin` piped in instead of an empty input.
fn with_stdin(mut command: Command, stdin: &str) -> Output {
    let mut child = command.stdin(Stdio::piped()).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn by_default_stdout_holds_only_the_reply_text() {
    let (port, _) = serve(vec![text_reply("hello from the fake model")]);
    let fixture = fixture(port);
    let output = command(&fixture, &["-p", "say hello"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "hello from the fake model\n"
    );
    assert!(!stderr.contains("\"v\""), "{stderr}");
}

#[test]
fn terminal_control_characters_in_the_reply_are_not_printed() {
    // OSC 52 would write to the clipboard of the terminal that shows it.
    let reply = "a\u{1b}]52;c;ZXZpbA==\u{7}b\tc\u{9b}31md\r\ne";
    let (port, _) = serve(vec![text_reply(reply)]);
    let fixture = fixture(port);
    let output = command(&fixture, &["-p", "say hello"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "a]52;c;ZXZpbA==b\tc31md\ne\n"
    );
}

#[test]
fn piped_stdin_comes_before_the_prompt_after_a_blank_line() {
    let (port, bodies) = serve(vec![text_reply("summary")]);
    let fixture = fixture(port);
    let output = with_stdin(
        command(&fixture, &["summarize this", "-p"]),
        "line one\nline two\n",
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        bodies.lock().unwrap()[0]["messages"][0]["content"],
        "line one\nline two\n\nsummarize this"
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "summary\n");
}

#[test]
fn piped_stdin_alone_is_the_prompt() {
    let (port, bodies) = serve(vec![text_reply("hi")]);
    let fixture = fixture(port);
    let output = with_stdin(command(&fixture, &["-p"]), "say hi\n");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        bodies.lock().unwrap()[0]["messages"][0]["content"],
        "say hi"
    );
}

#[test]
fn no_prompt_and_empty_stdin_is_a_usage_error() {
    let fixture = fixture(1);
    let output = command(&fixture, &["-p"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("the prompt is empty"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn json_output_is_the_finished_record_alone() {
    let (port, _) = serve(vec![text_reply("hello")]);
    let fixture = fixture(port);
    let output = command(&fixture, &["-p", "say hello", "--output-format=json"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = records(&output);
    assert_eq!(records.len(), 1, "{records:#?}");
    assert_eq!(records[0]["v"], 1);
    assert_eq!(records[0]["type"], "finished");
    assert_eq!(records[0]["ok"], true);
    assert_eq!(records[0]["text"], "hello");
}

#[test]
fn a_failed_turn_in_text_mode_reports_the_error_on_stderr() {
    let (port, _) = serve(vec![Reply::Events(vec![json!({
        "type": "error",
        "error": {"type": "invalid_request_error", "message": "the fake model refuses"}
    })])]);
    let fixture = fixture(port);
    let output = command(&fixture, &["-p", "say hello"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr
            .lines()
            .any(|line| line.starts_with("socium: ") && line.contains("the fake model refuses")),
        "{stderr}"
    );
}

/// The directory of `program` on the test's PATH, resolved to the store
/// path the sandbox accepts.
fn program_on_path(program: &str) -> std::path::PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("{program} is not on PATH"))
        .canonicalize()
        .unwrap()
}

#[test]
fn a_missing_ripgrep_is_named_at_startup() {
    let (port, _) = serve(vec![text_reply("hi")]);
    let fixture = fixture(port);
    let bin = fixture.root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    // Only bwrap, git, and the `true` the sandbox check runs, from their
    // own directories; no rg. A build sandbox may have no /usr/bin to fall
    // back on.
    let dirs = ["bwrap", "git", "true"]
        .map(|program| program_on_path(program).parent().unwrap().to_path_buf());
    let output = command(&fixture, &["-p", "hi"])
        .env("PATH", std::env::join_paths(dirs).unwrap())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    // The sandbox also looks in /usr/bin and /bin, whatever PATH says, so
    // a host that installed rg there still finds it.
    let in_system = ["/usr/bin/rg", "/bin/rg"]
        .iter()
        .any(|path| std::path::Path::new(path).exists());
    assert_eq!(
        stderr.contains("ripgrep (rg) is not installed"),
        !in_system,
        "{stderr}"
    );
    assert!(!stderr.contains("git is not installed"), "{stderr}");
}

#[test]
fn version_prints_the_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_socium"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("socium {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn client_bridge_keeps_approval_and_persisted_conversations_in_core() {
    let (port, _) = serve(vec![
        tool_reply("run_command", json!({"command": ["touch", "created"]})),
        text_reply("Done."),
    ]);
    let fixture = fixture(port);
    let mut child = command(&fixture, &["--client-bridge"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (send, receive) = std::sync::mpsc::channel();
    let reader = thread::spawn(move || {
        for line in stdout.lines() {
            let record: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if send.send(record).is_err() {
                break;
            }
        }
    });
    let next = |kind: &str| {
        loop {
            let record = receive
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap_or_else(|error| panic!("waiting for {kind}: {error}"));
            if record["type"] == kind {
                break record;
            }
        }
    };
    let initial = next("session");
    let session = initial["session"].as_str().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"id": "turn-1", "action": "prompt", "text": "Create a file."})
    )
    .unwrap();
    assert_eq!(next("running")["turn"], "turn-1");
    let approval = next("approval");
    assert_eq!(approval["tool"], "run_command");
    assert!(!fixture.workspace.join("created").exists());
    writeln!(
        stdin,
        "{}",
        json!({
            "id": "answer", "action": "approve", "request": approval["request"], "allow": true,
        })
    )
    .unwrap();
    let response = next("response");
    assert_eq!(response["id"], "answer");
    assert_eq!(response["error"], Value::Null);
    assert_eq!(next("delta")["text"], "Done.");
    let finished = next("session");
    assert_eq!(finished["session"], session);
    assert!(fixture.workspace.join("created").is_file());
    assert_eq!(
        finished["messages"].as_array().unwrap().last().unwrap()["text"],
        "Done."
    );

    writeln!(stdin, "{}", json!({"id": "fresh", "action": "new"})).unwrap();
    let fresh = next("session");
    assert_ne!(fresh["session"], session);
    assert_eq!(fresh["sessions"][0]["id"], session);
    writeln!(
        stdin,
        "{}",
        json!({"id": "reopen", "action": "resume", "session": session})
    )
    .unwrap();
    let resumed = next("session");
    assert_eq!(resumed["session"], session);
    assert_eq!(resumed["messages"], finished["messages"]);
    drop(stdin);
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
}
