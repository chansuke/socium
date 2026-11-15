//! Observation tests for the 2026-11-14 review. They describe behavior at
//! commit 2d4ac91 and are not regression tests: most print what happened
//! and assert the reviewed claim, so a later fix makes them fail.
//!
//! Each test is one observation, R1 to R16 (there is no R15); the review
//! document maps them to its findings. Not part of the crate. `run_repros.sh` copies this file into a temporary
//! export of the commit as `crates/core/src/review_repros_2026_11_14.rs` and
//! appends `#[cfg(test)] mod review_repros_2026_11_14;` to `lib.rs` there.
use super::*;
use crate::agent::types::{AgentMessageResponse, ContentBlock, MessageContent};
use crate::providers::ProviderResponse;
use crate::test_support::{ScriptedProvider, git_stdout, init_git_repo, text_provider_response};
use serde_json::json;
use std::time::Instant;

fn tool_calls(calls: &[(&str, serde_json::Value)]) -> ProviderResponse {
    ProviderResponse {
        message: AgentMessageResponse {
            content: calls
                .iter()
                .enumerate()
                .map(|(i, (name, input))| ContentBlock::ToolUse {
                    id: format!("call-{i}"),
                    name: (*name).into(),
                    input: input.clone(),
                })
                .collect(),
        },
        streamed_text: false,
        usage: None,
    }
}

fn scripted(
    root: &std::path::Path,
    model: &str,
    max_tokens: u32,
    responses: Vec<Result<ProviderResponse, ProviderError>>,
) -> (
    AgentService,
    std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
) {
    let provider = ScriptedProvider::new(responses);
    let requests = provider.recorded_requests.clone();
    (
        AgentService::with_provider(
            root.into(),
            Box::new(provider),
            model.into(),
            max_tokens,
            reqwest::Client::new(),
        ),
        requests,
    )
}

/// Runs one tool call with the approval answered "allow" at once.
fn approved(root: &std::path::Path, tool: &str, input: serde_json::Value) -> (bool, String) {
    let control = TaskControl::default();
    control.enable_interactive_approval();
    let result =
        crate::tools::run_tool_call_controlled(root, "call", tool, &input, &control, &mut |e| {
            if let AgentProgress::ApprovalRequested { request_id, .. } = e {
                control.answer_approval(request_id, true);
            }
        });
    match result {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => (is_error != Some(true), content),
        other => panic!("unexpected {other:?}"),
    }
}

fn local_anthropic(base_url: &str) -> crate::providers::anthropic::AnthropicProvider {
    let registry = crate::models::ModelRegistry::from_toml(&format!(
        "[providers.local]\nwire = \"anthropic\"\nbase_url = \"{base_url}\"\napi_key_env = \"LOCAL_KEY\"\n[[models]]\nalias = \"m\"\nprovider = \"local\"\nid = \"local-model\""
    ))
    .unwrap();
    crate::providers::anthropic::AnthropicProvider::from_config(
        registry.provider("local").unwrap(),
        registry.find("m").unwrap(),
        &|key: &str| (key == "LOCAL_KEY").then(|| "k".to_string()),
    )
    .unwrap()
}

// ---------------------------------------------------------------------
// R1: the HTTP client's 90 s timeout is a deadline for the whole response,
// so a healthy stream that keeps sending tokens is cut off.
// ---------------------------------------------------------------------
#[tokio::test]
#[ignore = "takes about 95 seconds"]
async fn review_r1_total_http_timeout_cuts_a_healthy_stream() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 65536];
        let _ = socket.read(&mut buffer).await.unwrap();
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let send = |event: serde_json::Value| format!("data: {event}\n\n");
        let _ = socket
            .write_all(
                (send(json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}))
                    + &send(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})))
                    .as_bytes(),
            )
            .await;
        // One token per second for 120 seconds: never idle, never stalled.
        for _ in 0..120 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if socket
                .write_all(
                    send(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}))
                        .as_bytes(),
                )
                .await
                .is_err()
            {
                return;
            }
        }
        let _ = socket
            .write_all(
                (send(json!({"type":"content_block_stop","index":0}))
                    + &send(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}))
                    + &send(json!({"type":"message_stop"})))
                .as_bytes(),
            )
            .await;
    });

    let provider = local_anthropic(&base_url);
    let messages = [Message::user_text("write a long file".into())];
    let request =
        crate::agent::types::AgentMessageRequest::new("local-model", 64_000, &messages, &[]);
    let started = Instant::now();
    let mut deltas = 0usize;
    let result = crate::providers::Provider::send_message(
        &provider,
        &crate::provider_http_client().unwrap(),
        &request,
        &mut |event| {
            if matches!(event, crate::providers::ProviderEvent::TextDelta { .. }) {
                deltas += 1;
            }
        },
    )
    .await;
    let elapsed = started.elapsed();
    let error = result.err();
    println!(
        "R1 elapsed={elapsed:?} deltas_received={deltas} shown to the user: {:?} debug: {error:?}",
        error.as_ref().map(ToString::to_string)
    );
    assert!(error.is_some(), "the 120 s stream completed");
    assert!(elapsed >= Duration::from_secs(85) && elapsed < Duration::from_secs(100));
    assert!(deltas >= 80, "tokens were arriving until the cut");
}

// ---------------------------------------------------------------------
// R2: edit_history.json keeps every before/after forever. Once it reaches
// the 64 MiB state limit every edit is refused, and /clear does not help.
// ---------------------------------------------------------------------
fn seed_edit_history(root: &std::path::Path, target_bytes: usize) -> std::path::PathBuf {
    let path = crate::agent::state::workspace_state_path(root, "edit_history.json").unwrap();
    // Records as apply_changes writes them: 100 KiB before and after.
    let body = "x".repeat(100 * 1024);
    let record = json!({"changes":[{"path":"old.rs","before":body,"after":body}],"complete":true})
        .to_string();
    let count = target_bytes / (record.len() + 1);
    let mut text = String::with_capacity(target_bytes + 64);
    text.push_str("{\"schema_version\":1,\"records\":[");
    for index in 0..count {
        if index > 0 {
            text.push(',');
        }
        text.push_str(&record);
    }
    // Pad with one more record so the file ends a few hundred bytes short
    // of `target_bytes`.
    let overhead = json!({"changes":[{"path":"old.rs","before":"","after":""}],"complete":true})
        .to_string()
        .len()
        + 3;
    let remaining = target_bytes.saturating_sub(text.len() + overhead + 256);
    if count > 0 && remaining > 0 {
        text.push(',');
        text.push_str(
            &json!({"changes":[{"path":"old.rs","before":"x".repeat(remaining),"after":""}],"complete":true})
                .to_string(),
        );
    }
    text.push_str("]}");
    std::fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path
}

#[test]
fn review_r2_edit_history_growth_slows_then_blocks_every_edit() {
    let edit = |root: &std::path::Path| {
        let started = Instant::now();
        let result = crate::tools::run_tool_call(
            root,
            "edit",
            crate::constants::TEXT_EDITOR_TOOL_NAME,
            &json!({"command":"str_replace","path":"small.txt","old_str":"old","new_str":"new"}),
        );
        (started.elapsed(), result)
    };

    // Baseline: an empty history.
    let fresh = tempfile::tempdir().unwrap();
    std::fs::write(fresh.path().join("small.txt"), "old\n").unwrap();
    let (baseline, result) = edit(fresh.path());
    assert!(matches!(
        result,
        ContentBlock::ToolResult { is_error: None, .. }
    ));

    // Half way to the limit: the edit still works but pays for the history.
    let half = tempfile::tempdir().unwrap();
    std::fs::write(half.path().join("small.txt"), "old\n").unwrap();
    let path = seed_edit_history(half.path(), 30 * 1024 * 1024);
    let seeded = std::fs::metadata(&path).unwrap().len();
    let (slow, result) = edit(half.path());
    assert!(
        matches!(result, ContentBlock::ToolResult { is_error: None, .. }),
        "{result:?}"
    );
    println!(
        "R2 one 4-byte edit: empty history {baseline:?}; {seeded} byte history {slow:?}; history now {} bytes",
        std::fs::metadata(&path).unwrap().len()
    );

    // At the limit: the edit is refused and the file is untouched.
    let full = tempfile::tempdir().unwrap();
    std::fs::write(full.path().join("small.txt"), "old\n").unwrap();
    let path = seed_edit_history(full.path(), 64 * 1024 * 1024);
    println!(
        "R2 seeded history: {} bytes",
        std::fs::metadata(&path).unwrap().len()
    );
    let (_, result) = edit(full.path());
    println!("R2 edit at the limit -> {result:?}");
    assert!(matches!(
        result,
        ContentBlock::ToolResult {
            is_error: Some(true),
            ..
        }
    ));
    assert_eq!(
        std::fs::read_to_string(full.path().join("small.txt")).unwrap(),
        "old\n"
    );

    // /clear resets the conversation, not the edit history.
    let (mut service, _) = scripted(full.path(), "test", 1000, vec![]);
    service.clear().unwrap();
    let (_, result) = edit(full.path());
    assert!(
        matches!(
            result,
            ContentBlock::ToolResult {
                is_error: Some(true),
                ..
            }
        ),
        "after /clear: {result:?}"
    );
    // apply_patch shares the same history.
    let result = crate::tools::run_tool_call(
        full.path(),
        "patch",
        "apply_patch",
        &json!({"patch":"*** Begin Patch\n*** Add File: new.txt\n+x\n*** End Patch"}),
    );
    println!("R2 apply_patch at the limit -> {result:?}");
    assert!(!full.path().join("new.txt").exists());
}

// ---------------------------------------------------------------------
// R3: large tool results in the newest messages cannot be compacted or
// trimmed, so the session cannot continue.
// ---------------------------------------------------------------------
#[tokio::test]
async fn review_r3_large_parallel_reads_strand_the_session() {
    let root = tempfile::tempdir().unwrap();
    let line = "let value = compute(input, 1234567890); // filler line\n";
    for name in ["a.rs", "b.rs", "c.rs"] {
        std::fs::write(
            root.path().join(name),
            line.repeat(199_000 / line.len()),
        )
        .unwrap();
    }
    let reads = tool_calls(&[
        ("read_file", json!({"path":"a.rs"})),
        ("read_file", json!({"path":"b.rs"})),
        ("read_file", json!({"path":"c.rs"})),
    ]);
    let mut responses = vec![Ok(reads)];
    responses.extend((0..20).map(|i| Ok(text_provider_response(&format!("answer {i}"), None))));
    // Defaults of the built-in model: 200k window, 64k output allowance.
    let (mut service, requests) = scripted(root.path(), "claude-opus-5-5", 64_000, responses);
    // Five earlier, small turns: an ordinary session, not a fresh one.
    for turn in 0..5 {
        service.messages.push(Message::user_text(format!("earlier question {turn}")));
        service.messages.push(Message::assistant_blocks(vec![ContentBlock::Text {
            text: format!("earlier answer {turn}"),
        }]));
    }

    let first = service.submit("Compare a.rs, b.rs and c.rs".into()).await;
    println!("R3 first turn -> {:?}", first.as_ref().err().map(ToString::to_string));
    assert!(
        first
            .err()
            .unwrap()
            .to_string()
            .contains("Context budget exceeded during tool loop")
    );
    assert_eq!(requests.lock().unwrap().len(), 1, "the reads were never shown to the model");

    let compact = service.compact().await;
    println!("R3 /compact -> {:?}", compact.as_ref().err().map(ToString::to_string));
    assert!(matches!(compact, Err(AgentError::NothingToCompact)));

    let mut failures = 0;
    let requests_before = requests.lock().unwrap().len();
    loop {
        match service.submit("continue".into()).await {
            Ok(reply) => {
                println!("R3 recovered after {failures} failed submissions: {}", reply.text);
                break;
            }
            Err(err) => {
                failures += 1;
                if failures == 1 {
                    println!("R3 next turn -> {err}");
                    assert!(err.to_string().contains("Context exceeds input budget"));
                }
                assert!(failures < 30, "never recovered");
            }
        }
    }
    let summary = service.summary_state.summary.as_ref().unwrap();
    println!(
        "R3 after recovery: covered_messages={} of {}; summary chars={}; summary mentions file body: {}",
        summary.covered_messages,
        service.messages.len(),
        summary.text.chars().count(),
        summary.text.contains("compute(input")
    );
    let after = requests.lock().unwrap()[requests_before].to_string();
    println!(
        "R3 first accepted request: chars={} contains a file body: {} contains the original request: {}",
        after.chars().count(),
        after.contains("compute(input"),
        after.contains("Compare a.rs, b.rs and c.rs")
    );
    assert!(failures >= 3, "expected several dead submissions, got {failures}");
    assert!(!after.contains("compute(input"));
}

// ---------------------------------------------------------------------
// R4: only /model strips thinking. A model chosen by MODEL or the registry
// default is not recorded, so its thinking goes to the next model.
// ---------------------------------------------------------------------
#[test]
fn review_r4_model_env_change_sends_old_thinking_to_another_provider() {
    use std::collections::HashMap;
    use std::sync::Arc;
    const MODELS: &str = r#"
[providers.deepseek]
wire = "anthropic"
base_url = "https://api.deepseek.com/anthropic"
api_key_env = "DEEPSEEK_API_KEY"
auth = "bearer"

[[models]]
alias = "ds-flash"
provider = "deepseek"
id = "deepseek-flash"
"#;
    let open = |root: &std::path::Path, model: &str| {
        let map: HashMap<String, String> = [
            ("ANTHROPIC_API_KEY", "a"),
            ("DEEPSEEK_API_KEY", "d"),
            ("MODEL", model),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        AgentService::open(
            root.to_path_buf(),
            SessionStart::Continue,
            crate::models::ModelRegistry::from_toml(MODELS).unwrap(),
            Arc::new(move |key| map.get(key).cloned()),
            None,
            reqwest::Client::new(),
        )
        .unwrap()
    };
    let root = tempfile::tempdir().unwrap();
    let mut service = open(root.path(), "opus");
    service.messages = vec![
        Message::user_text("hi".into()),
        Message::assistant_blocks(vec![
            ContentBlock::Thinking {
                thinking: "plan".into(),
                signature: "anthropic-signature".into(),
            },
            ContentBlock::RedactedThinking {
                data: "anthropic-redacted".into(),
            },
            ContentBlock::Text {
                text: "answer".into(),
            },
        ]),
    ];
    service.persist_state().unwrap();
    drop(service);

    let service = open(root.path(), "ds-flash");
    assert_eq!(service.active_model().provider, "deepseek");
    let prepared = crate::agent::context::prepare_context_messages(
        &service.messages,
        &service.summary_state,
        service.active_model.context_window,
        &service.active_model.id,
        service.max_tokens,
        &service.tools,
    )
    .unwrap();
    let request = crate::agent::types::AgentMessageRequest::new(
        &service.active_model.id,
        service.max_tokens,
        &prepared.messages,
        &service.tools,
    );
    let body = serde_json::to_string(&request).unwrap();
    println!(
        "R4 request to {} contains anthropic signature: {}, redacted data: {}",
        service.active_model().provider,
        body.contains("anthropic-signature"),
        body.contains("anthropic-redacted")
    );
    assert!(body.contains("anthropic-signature") && body.contains("anthropic-redacted"));
}

// ---------------------------------------------------------------------
// R5: the compaction model sees only the first 600 characters of each
// message, so the end of a long request never reaches the summary.
// ---------------------------------------------------------------------
#[tokio::test]
async fn review_r5_compaction_never_sees_the_end_of_a_long_request() {
    let root = tempfile::tempdir().unwrap();
    let mut request = "Background paragraph. ".repeat(60);
    request.push_str("HARD REQUIREMENT: never touch the public API of crate foo.");
    let mut messages = vec![Message::user_text(request)];
    messages.extend((0..30).map(|i| Message::user_text(format!("message {i}"))));
    let (mut service, requests) = scripted(
        root.path(),
        "claude-opus-5-5",
        1000,
        vec![Ok(text_provider_response("1. User request: summary", None))],
    );
    service.messages = messages;
    let report = service.compact().await.unwrap();
    assert!(report.used_llm);
    let prompt = requests.lock().unwrap()[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    println!(
        "R5 compaction prompt chars={} contains the requirement: {}",
        prompt.chars().count(),
        prompt.contains("HARD REQUIREMENT")
    );
    assert!(!prompt.contains("HARD REQUIREMENT"));
    // And the turn after compaction does not carry it either.
    let prepared = crate::agent::context::prepare_context_messages(
        &service.messages,
        &service.summary_state,
        None,
        "claude-opus-5-5",
        1000,
        &service.tools,
    )
    .unwrap();
    assert!(
        !serde_json::to_string(&prepared.messages)
            .unwrap()
            .contains("HARD REQUIREMENT")
    );
}

// ---------------------------------------------------------------------
// R6: what a Rust project can do inside the sandbox.
// ---------------------------------------------------------------------
#[test]
fn review_r6_rust_build_inside_the_sandbox() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("main.rs"),
        "fn main() { println!(\"hello\"); }\n",
    )
    .unwrap();
    std::fs::write(root.path().join("t.c"), "int main(void){return 0;}\n").unwrap();
    std::fs::create_dir_all(root.path().join("proj/src")).unwrap();
    std::fs::write(
        root.path().join("proj/Cargo.toml"),
        "[package]\nname = \"proj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nitoa = \"1\"\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("proj/src/main.rs"),
        "fn main() { println!(\"{}\", itoa::Buffer::new().format(1)); }\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.path().join("nodeps/src")).unwrap();
    std::fs::write(
        root.path().join("nodeps/Cargo.toml"),
        "[package]\nname = \"nodeps\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("nodeps/src/main.rs"),
        "fn main() { println!(\"ok\"); }\n",
    )
    .unwrap();

    let run = |label: &str, input: serde_json::Value| {
        let (ok, content) = approved(root.path(), "run_command", input);
        let tail: String = content
            .lines()
            .filter(|l| !l.starts_with("Artifacts:") && !l.contains("PATH="))
            .collect::<Vec<_>>()
            .join("\n");
        println!("R6 [{label}] ok={ok}\n{}\n", tail.chars().take(1400).collect::<String>());
        ok
    };
    run("rustc --version", json!({"command":["rustc","--version"]}));
    // The sandbox PATH keeps only system runtime directories, so a version
    // manager under $HOME is skipped and another install answers.
    run("node --version", json!({"command":["node","--version"]}));
    run("python3 --version", json!({"command":["python3","--version"]}));
    run("cc t.c", json!({"command":["sh","-c","cc t.c -o t && ./t && echo c-ok"]}));
    run("rustc main.rs", json!({"command":["rustc","main.rs","-o","hello"],"timeout_ms":120000}));
    run(
        "python3 subprocess",
        json!({"command":["python3","-c","import subprocess; print(subprocess.run(['echo','sub-ok'],capture_output=True,text=True).stdout)"]}),
    );
    run(
        "python3 asyncio",
        json!({"command":["python3","-c","import asyncio; asyncio.run(asyncio.sleep(0)); print('asyncio-ok')"]}),
    );
    run(
        "python3 multiprocessing",
        json!({"command":["python3","-c","import multiprocessing as m\nif __name__=='__main__':\n    with m.Pool(2) as p: print(p.map(abs,[-1,-2]))"]}),
    );
    run(
        "node child_process",
        json!({"command":["node","-e","console.log(require('child_process').execSync('echo node-ok').toString())"]}),
    );
    run(
        "git status (plain command)",
        json!({"command":["git","--version"]}),
    );
    run(
        "cargo build (no deps)",
        json!({"command":["cargo","build"],"cwd":"nodeps","timeout_ms":120000}),
    );
    run(
        "cargo build (crates.io dep)",
        json!({"command":["cargo","build"],"cwd":"proj","timeout_ms":120000}),
    );
    run(
        "cargo build --offline (crates.io dep)",
        json!({"command":["cargo","build","--offline"],"cwd":"proj","timeout_ms":120000}),
    );
}

// ---------------------------------------------------------------------
// R7: git_commit and files the agent created; identity from global config.
// ---------------------------------------------------------------------
#[test]
fn review_r7_git_commit_limits() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    let (ok, _) = approved(
        root.path(),
        crate::constants::TEXT_EDITOR_TOOL_NAME,
        json!({"command":"create","path":"src/new_module.rs","file_text":"pub fn f() {}\n"}),
    );
    assert!(ok);
    let (ok, content) = approved(
        root.path(),
        "git_commit",
        json!({"message":"add module","paths":["src/new_module.rs"]}),
    );
    println!("R7 commit of an agent-created file: ok={ok}\n{content}\n");
    assert!(!ok);
    // The model cannot stage it either: .git is read-only for commands.
    let (ok, content) = approved(
        root.path(),
        "run_command",
        json!({"command":["git","add","src/new_module.rs"]}),
    );
    let tail = content
        .lines()
        .filter(|l| !l.starts_with("Artifacts:"))
        .collect::<Vec<_>>()
        .join("\n");
    println!("R7 git add through run_command: ok={ok}\n{tail}\n");
    assert!(!ok);

    // A repository whose identity lives only in the user's global config.
    let other = tempfile::tempdir().unwrap();
    crate::test_support::run_git(other.path(), &["init", "-b", "main"]);
    std::fs::write(other.path().join("tracked.txt"), "one\n").unwrap();
    crate::test_support::run_git(other.path(), &["add", "tracked.txt"]);
    crate::test_support::run_git(
        other.path(),
        &[
            "-c",
            "user.name=Human",
            "-c",
            "user.email=human@example.com",
            "commit",
            "-m",
            "initial",
        ],
    );
    std::fs::write(other.path().join("tracked.txt"), "two\n").unwrap();
    let (ok, content) = approved(
        other.path(),
        "git_commit",
        json!({"message":"change","paths":["tracked.txt"]}),
    );
    println!("R7 commit without repository-local identity: ok={ok}\n{content}\n");
    assert!(!ok);
    assert_eq!(
        git_stdout(other.path(), &["log", "--oneline"]).lines().count(),
        1
    );
}

// ---------------------------------------------------------------------
// R8: recursive listing does not skip .git or ignored build output.
// ---------------------------------------------------------------------
#[test]
fn review_r8_recursive_listing_is_filled_by_git_and_build_output() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    std::fs::write(root.path().join(".gitignore"), "/target\n").unwrap();
    std::fs::create_dir_all(root.path().join("target/debug/deps")).unwrap();
    for index in 0..1500 {
        std::fs::write(
            root.path().join(format!("target/debug/deps/unit-{index:04}.o")),
            "",
        )
        .unwrap();
    }
    let output = crate::tools::list_files::list_files_from_workspace(
        root.path(),
        &json!({"recursive": true}),
    )
    .unwrap();
    let lines = output.lines().collect::<Vec<_>>();
    println!(
        "R8 lines={} .git entries={} target entries={} lists src/lib.rs={} last line: {}",
        lines.len(),
        lines.iter().filter(|l| l.contains("] .git/")).count(),
        lines.iter().filter(|l| l.contains("] target/")).count(),
        output.contains("src/lib.rs"),
        lines.last().unwrap()
    );
    assert!(!output.contains("src/lib.rs"));
    // Without build output the same call lists Git's internal files.
    std::fs::remove_dir_all(root.path().join("target")).unwrap();
    let output = crate::tools::list_files::list_files_from_workspace(
        root.path(),
        &json!({"recursive": true}),
    )
    .unwrap();
    let lines = output.lines().collect::<Vec<_>>();
    println!(
        "R8 without target: lines={} .git entries={} lists src/lib.rs={}",
        lines.len(),
        lines.iter().filter(|l| l.contains("] .git/")).count(),
        output.contains("src/lib.rs")
    );
    assert!(lines.iter().any(|l| l.contains("] .git/")));
}

// ---------------------------------------------------------------------
// R9: an event type the parser does not know fails the whole response.
// ---------------------------------------------------------------------
#[tokio::test]
async fn review_r9_unknown_stream_event_fails_the_response() {
    let events = [
        json!({"type":"message_start","message":{"usage":{"input_tokens":1}}}),
        json!({"type":"some_future_event","detail":{}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"hello"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_stop"}),
    ];
    let body = events
        .iter()
        .map(|event| format!("event: {}\ndata: {event}\n\n", event["type"].as_str().unwrap()))
        .collect::<String>();
    let (base_url, _server) = crate::providers::test_server::serve_once("200 OK", body).await;
    let provider = local_anthropic(&base_url);
    let messages = [Message::user_text("hi".into())];
    let request = crate::agent::types::AgentMessageRequest::new("local-model", 100, &messages, &[]);
    let result = crate::providers::Provider::send_message(
        &provider,
        &crate::provider_http_client().unwrap(),
        &request,
        &mut |_| {},
    )
    .await;
    let error = result.err().map(|e| e.to_string());
    println!("R9 unknown event -> {error:?}");
    assert!(error.is_some());
}

// ---------------------------------------------------------------------
// R10: the prompt asks for path:line, the read tools return no line numbers.
// ---------------------------------------------------------------------
#[test]
fn review_r10_read_tools_return_no_line_numbers() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("f.rs"), "fn a() {}\nfn b() {}\n").unwrap();
    let (_, read) = approved(root.path(), "read_file", json!({"path":"f.rs"}));
    let (_, view) = approved(
        root.path(),
        crate::constants::TEXT_EDITOR_TOOL_NAME,
        json!({"command":"view","path":"f.rs","view_range":[2,2]}),
    );
    println!("R10 read_file -> {read:?}\nR10 view [2,2] -> {view:?}");
    assert_eq!(read, "Path: f.rs\nfn a() {}\nfn b() {}\n");
    assert_eq!(view, "Path: f.rs\nfn b() {}\n");
    assert!(crate::constants::DEFAULT_SYSTEM_PROMPT.contains("path:line"));
}

// ---------------------------------------------------------------------
// R11: at the 64 MiB snapshot limit the conversation cannot continue.
// ---------------------------------------------------------------------
#[tokio::test]
async fn review_r11_conversation_limit_leaves_only_clear() {
    let root = tempfile::tempdir().unwrap();
    let (mut service, _) = scripted(
        root.path(),
        "claude-opus-5-5",
        1000,
        vec![Ok(text_provider_response("answer", None))],
    );
    service.messages = (0..7)
        .map(|_| Message::user_text("y".repeat(10 * 1024 * 1024)))
        .collect();
    let before = service.messages.len();
    let result = service.submit("next".into()).await;
    println!(
        "R11 submit at the limit -> {:?}; messages in memory {} -> {}",
        result.as_ref().err().map(ToString::to_string),
        before,
        service.messages.len()
    );
    assert!(result.err().unwrap().to_string().contains("64 MiB"));
    let old = service.session_id().to_string();
    assert!(service.clear().is_ok());
    assert!(service.messages.is_empty());
    println!(
        "R11 after /clear: new session {}, old session {old} has a saved snapshot: {}",
        service.session_id(),
        crate::session::session_file(root.path(), &old, "conversation.json").exists()
    );
}

// ---------------------------------------------------------------------
// R12 (claim dropped): context metrics are recomputed from the whole
// history on each key. Measured in a release build it only matters for an
// unsummarized history far over the budget, which a session does not keep.
// ---------------------------------------------------------------------
#[test]
fn review_r12_context_metrics_cost_per_keystroke() {
    let root = tempfile::tempdir().unwrap();
    let (mut service, _) = scripted(root.path(), "claude-opus-5-5", 64_000, vec![]);
    for megabytes in [1usize, 8, 32] {
        service.messages = (0..megabytes * 20)
            .map(|_| Message::user_text("z".repeat(50 * 1024)))
            .collect();
        let started = Instant::now();
        for _ in 0..3 {
            service.context_metrics("typed text").unwrap();
        }
        println!(
            "R12 history {megabytes} MiB, no summary: context_metrics takes {:?} per call",
            started.elapsed() / 3
        );
        // The usual long session: a summary covers all but the last 16.
        service.summary_state = crate::agent::state::ConversationSummaryState {
            summary: Some(crate::agent::state::ConversationSummary {
                covered_messages: service.messages.len() - 16,
                text: "summary".into(),
                thinking_valid_from: Some(service.messages.len()),
            }),
        };
        for message in service.messages.iter_mut().rev().take(16) {
            *message = Message::user_text("short".into());
        }
        let started = Instant::now();
        for _ in 0..3 {
            service.context_metrics("typed text").unwrap();
        }
        println!(
            "R12 history {megabytes} MiB, summarized: context_metrics takes {:?} per call",
            started.elapsed() / 3
        );
        service.summary_state = Default::default();
    }
}

// ---------------------------------------------------------------------
// R13: patch format details models commonly emit.
// ---------------------------------------------------------------------
#[test]
fn review_r13_patch_format_edges() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("f.txt"), "one\ntwo\n").unwrap();
    let apply = |patch: &str| {
        crate::tools::apply_patch::apply_patch_from_workspace(root.path(), &json!({"patch": patch}))
            .map_err(|e| e.to_string())
    };
    println!(
        "R13 end-of-file marker -> {:?}",
        apply("*** Begin Patch\n*** Update File: f.txt\n@@\n one\n-two\n+TWO\n*** End of File\n*** End Patch")
    );
    println!(
        "R13 blank context line -> {:?}",
        apply("*** Begin Patch\n*** Update File: f.txt\n@@\n one\n\n-two\n+TWO\n*** End Patch")
    );
    apply("*** Begin Patch\n*** Add File: created.txt\n+hello\n*** End Patch").unwrap();
    let created = std::fs::read(root.path().join("created.txt")).unwrap();
    println!("R13 Add File bytes -> {:?}", String::from_utf8_lossy(&created));
    assert_eq!(created, b"hello");
    // The dry run of a one-line change returns both whole files.
    let big = "line\n".repeat(4000) + "target\n";
    std::fs::write(root.path().join("big.txt"), &big).unwrap();
    let preview = crate::tools::apply_patch::apply_patch_from_workspace(
        root.path(),
        &json!({"dry_run": true, "patch":"*** Begin Patch\n*** Update File: big.txt\n@@\n-target\n+TARGET\n*** End Patch"}),
    )
    .unwrap();
    println!(
        "R13 dry run of a 1-line change in a {}-byte file returns {} bytes",
        big.len(),
        preview.len()
    );
    assert!(preview.len() > 2 * big.len());
}

// ---------------------------------------------------------------------
// R14: a linked Git worktree cannot be a workspace at all.
// ---------------------------------------------------------------------
#[test]
fn review_r14_linked_worktree_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main");
    std::fs::create_dir(&main).unwrap();
    init_git_repo(&main);
    let linked = root.path().join("linked");
    crate::test_support::run_git(
        &main,
        &["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
    );
    let probe = crate::process::run(
        &["true".into()],
        &linked,
        Duration::from_secs(5),
        &TaskControl::default(),
    );
    println!(
        "R14 startup probe in a linked worktree -> {:?}",
        probe.as_ref().err().map(ToString::to_string)
    );
    assert!(probe.is_err());
    let (ok, content) = approved(&linked, "git_status", json!({}));
    println!("R14 git_status in a linked worktree: ok={ok} {content}");
    assert!(!ok);
}

// ---------------------------------------------------------------------
// R16: file tools need no approval, also for files the host runs later,
// and read tools return secrets kept in the workspace.
// ---------------------------------------------------------------------
#[test]
fn review_r16_unapproved_edits_reach_host_executed_files() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(".envrc"),
        "export ANTHROPIC_API_KEY='placeholder-not-a-key'\n",
    )
    .unwrap();
    std::fs::write(root.path().join("build.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
    std::fs::write(root.path().join(".github/workflows/ci.yaml"), "on: push\n").unwrap();
    // No interactive approval is installed: anything that asks is denied.
    let control = TaskControl::default();
    let run = |tool: &str, input: serde_json::Value| match crate::tools::run_tool_call_controlled(
        root.path(),
        "c",
        tool,
        &input,
        &control,
        &mut |_| {},
    ) {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => (is_error != Some(true), content),
        other => panic!("{other:?}"),
    };
    let (ok, content) = run("read_file", json!({"path": ".envrc"}));
    println!(
        "R16 read_file .envrc without approval: ok={ok} returns the key: {}",
        content.contains("placeholder-not-a-key")
    );
    assert!(ok && content.contains("placeholder-not-a-key"));
    for (path, old, new) in [
        (".envrc", "export ANTHROPIC_API_KEY='placeholder-not-a-key'", "echo changed"),
        ("build.rs", "fn main() {}", "fn main() { /* changed */ }"),
        (".github/workflows/ci.yaml", "on: push", "on: pull_request_target"),
    ] {
        let (ok, _) = run(
            crate::constants::TEXT_EDITOR_TOOL_NAME,
            json!({"command":"str_replace","path":path,"old_str":old,"new_str":new}),
        );
        println!("R16 edit {path} without approval: ok={ok}");
        assert!(ok);
    }
    let (ok, _) = run("run_command", json!({"command":["true"]}));
    println!("R16 run_command without approval: ok={ok}");
    assert!(!ok);
}

#[allow(dead_code)]
fn unused(_: &MessageContent) {}
