//! Offline acceptance scenarios for everyday work, not provider-quality evals.
use super::*;
use crate::agent::types::{AgentMessageResponse, ContentBlock, MessageContent};
use crate::providers::{ProviderCapabilities, ProviderEvent, ProviderKind, ProviderResponse};
use crate::test_support::{ScriptedProvider, git_stdout, init_git_repo, text_provider_response};
use serde_json::json;

fn response(calls: &[(&str, serde_json::Value)]) -> ProviderResponse {
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
fn service(
    root: &std::path::Path,
    responses: Vec<Result<ProviderResponse, ProviderError>>,
) -> AgentService {
    AgentService::with_provider(
        root.into(),
        Box::new(ScriptedProvider::new(responses)),
        "test".into(),
        1000,
        reqwest::Client::new(),
    )
}

#[tokio::test]
async fn read_search_edit_test_failure_restart_and_resume_preserve_human_changes() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    std::fs::write(root.path().join("tracked.txt"), "human line\nold\n").unwrap();
    std::fs::write(root.path().join("staged.txt"), "staged human work\n").unwrap();
    git_stdout(root.path(), &["add", "staged.txt"]);
    std::fs::write(root.path().join("notes.txt"), "untracked human work\n").unwrap();
    let index_before = std::fs::read(root.path().join(".git/index")).unwrap();
    let calls = response(&[
        ("read_file", json!({"path":"tracked.txt"})),
        ("search_text", json!({"pattern":"old","path":"tracked.txt"})),
        (
            "apply_patch",
            json!({"patch":"*** Begin Patch\n*** Update File: tracked.txt\n@@\n-old\n+new\n*** End Patch"}),
        ),
        (
            "run_command",
            json!({"command":["sh","-c","test \"$(cat tracked.txt)\" = 'human line\nnew'"]}),
        ),
    ]);
    let mut agent = service(
        root.path(),
        vec![Ok(calls), Err(ProviderError::Api("connection lost".into()))],
    );
    let control = agent.task_control();
    control.enable_interactive_approval();
    let mut live_tools = Vec::new();
    assert!(
        agent
            .submit_with_progress("Update old to new and test".into(), |event| {
                match event {
                    AgentProgress::ApprovalRequested { request_id, .. } => {
                        control.answer_approval(request_id, true);
                    }
                    AgentProgress::ToolCallRecorded(entry) => live_tools.push(entry),
                    _ => {}
                }
            })
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("tracked.txt")).unwrap(),
        "human line\nnew\n"
    );
    assert_eq!(
        std::fs::read(root.path().join(".git/index")).unwrap(),
        index_before
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("notes.txt")).unwrap(),
        "untracked human work\n"
    );
    drop(agent);
    let mut restarted = service(
        root.path(),
        vec![Ok(text_provider_response(
            "Resumed with saved results",
            None,
        ))],
    );
    restarted.restore_state().unwrap();
    let all_results: Vec<_> = restarted
        .messages
        .iter()
        .flat_map(|m| match &m.content {
            MessageContent::Blocks(b) => b.as_slice(),
            _ => &[],
        })
        .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
        .collect();
    assert_eq!(all_results.len(), 4);
    assert!(
        all_results
            .iter()
            .all(|b| matches!(b, ContentBlock::ToolResult { is_error: None, .. }))
    );
    // The restarted transcript shows each call with its outcome and the
    // turn's failure.
    let entries = restarted.transcript_entries();
    let tools = entries
        .iter()
        .filter(|e| matches!(e.role, crate::TranscriptRole::Tool { failed: false }))
        .map(|e| e.text.lines().next().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(tools.len(), 4, "{entries:?}");
    // The live transcript showed the same entries.
    assert_eq!(
        live_tools,
        entries
            .iter()
            .filter(|e| matches!(e.role, crate::TranscriptRole::Tool { .. }))
            .cloned()
            .collect::<Vec<_>>()
    );
    assert!(tools[2].starts_with("apply_patch tracked.txt"));
    assert!(
        tools[3].starts_with("run_command sh -c") && tools[3].ends_with("(approved)"),
        "{tools:?}"
    );
    assert_eq!(
        entries.last().unwrap().text,
        "Request failed: connection lost"
    );
    assert_eq!(
        restarted
            .submit("Continue from saved results".into())
            .await
            .unwrap()
            .text,
        "Resumed with saved results"
    );
}

#[tokio::test]
async fn command_cancel_persists_outcome_and_skips_remaining_calls() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = service(
        root.path(),
        vec![Ok(response(&[
            (
                "run_command",
                json!({"command":["sh","-c","sleep 30"],"timeout_ms":60000}),
            ),
            (
                "apply_patch",
                json!({"patch":"*** Begin Patch\n*** Add File: must-not-exist\n+bad\n*** End Patch"}),
            ),
        ]))],
    );
    let control = agent.task_control();
    control.enable_interactive_approval();
    let start = std::time::Instant::now();
    let mut cancel_thread = None;
    let result = agent
        .submit_with_progress("Test then edit".into(), |event| match event {
            AgentProgress::ApprovalRequested { request_id, .. } => {
                control.answer_approval(request_id, true);
            }
            AgentProgress::CommandStarted { .. } => {
                let c = control.clone();
                cancel_thread = Some(std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(50));
                    c.cancel();
                }));
            }
            _ => {}
        })
        .await;
    cancel_thread.unwrap().join().unwrap();
    assert!(matches!(result, Err(AgentError::Cancelled)));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!root.path().join("must-not-exist").exists());
    drop(agent);
    let mut restarted = service(root.path(), vec![]);
    restarted.restore_state().unwrap();
    let history = serde_json::to_string(&restarted.messages).unwrap();
    assert!(history.contains("command cancelled"));
    assert!(history.contains("Cancelled before execution; no side effects"));
}

struct WaitingProvider;
#[async_trait::async_trait]
impl Provider for WaitingProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_tools: true,
        }
    }
    async fn send_message(
        &self,
        _: &reqwest::Client,
        _: &crate::agent::types::AgentMessageRequest<'_>,
        _: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<ProviderResponse, ProviderError> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn http_wait_is_cancellable_and_user_request_survives() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = AgentService::with_provider(
        root.path().into(),
        Box::new(WaitingProvider),
        "test".into(),
        1000,
        reqwest::Client::new(),
    );
    let c = agent.task_control();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        c.cancel();
    });
    let result = agent.submit("Keep this request".into()).await;
    worker.join().unwrap();
    assert!(matches!(result, Err(AgentError::Cancelled)));
    drop(agent);
    let mut restarted = service(root.path(), vec![]);
    restarted.restore_state().unwrap();
    assert_eq!(restarted.transcript_entries()[0].text, "Keep this request");
}

#[test]
fn unfinished_intent_is_unknown_and_not_reexecuted() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = service(root.path(), vec![]);
    agent.ensure_state_lock().unwrap();
    agent.messages.push(Message::assistant_blocks(
        response(&[(
            "run_command",
            json!({"command":["touch","should-not-exist"]}),
        )])
        .message
        .content,
    ));
    agent.persist_state().unwrap();
    drop(agent);
    let mut restarted = service(root.path(), vec![]);
    restarted.restore_state().unwrap();
    assert!(!root.path().join("should-not-exist").exists());
    assert!(
        serde_json::to_string(&restarted.messages)
            .unwrap()
            .contains("Outcome unknown")
    );
}

#[test]
fn competing_writer_corruption_and_unknown_schema_do_not_erase_history() {
    let root = tempfile::tempdir().unwrap();
    let mut first = service(root.path(), vec![]);
    first.restore_state().unwrap();
    let mut second = service(root.path(), vec![]);
    assert!(second.restore_state().is_err());
    let path = first.conversation_path();
    drop(first);
    drop(second);
    for bytes in [
        b"{broken".as_slice(),
        b"{\"schema_version\":999,\"messages\":[]}",
    ] {
        std::fs::write(&path, bytes).unwrap();
        let mut agent = service(root.path(), vec![]);
        assert!(agent.restore_state().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn failed_initial_checkpoint_prevents_tools() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = service(
        root.path(),
        vec![Ok(response(&[(
            "apply_patch",
            json!({"patch":"*** Begin Patch\n*** Add File: never\n+bad\n*** End Patch"}),
        )]))],
    );
    std::fs::create_dir_all(agent.conversation_path()).unwrap();
    assert!(agent.submit("Do work".into()).await.is_err());
    assert!(!root.path().join("never").exists());
}

#[test]
fn metadata_paths_and_aliases_are_rejected_before_editing() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".socium")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(".socium", root.path().join("alias")).unwrap();
    for path in [".socium/injected", ".git/config", "alias/injected"] {
        let patch = format!("*** Begin Patch\n*** Add File: {path}\n+bad\n*** End Patch");
        for dry_run in [true, false] {
            assert!(
                crate::tools::apply_patch::apply_patch_from_workspace(
                    root.path(),
                    &json!({"patch":patch,"dry_run":dry_run})
                )
                .is_err()
            );
        }
    }
    assert!(!root.path().join(".socium/injected").exists());
    assert!(!root.path().join(".socium/edit_history.json").exists());
}

#[test]
fn staged_only_commit_and_changed_approval_scope() {
    let root = tempfile::tempdir().unwrap();
    init_git_repo(root.path());
    std::fs::write(root.path().join("requested.txt"), "staged\n").unwrap();
    git_stdout(root.path(), &["add", "requested.txt"]);
    std::fs::write(root.path().join("unrelated.txt"), "must remain untracked\n").unwrap();
    let control = TaskControl::default();
    control.enable_interactive_approval();
    let commit = |callback: &mut dyn FnMut(AgentProgress)| {
        crate::tools::run_tool_call_controlled(
            root.path(),
            "commit",
            "git_commit",
            &json!({"message":"requested only"}),
            &control,
            &mut |e| callback(e),
        )
    };
    let result = commit(&mut |e| {
        if let AgentProgress::ApprovalRequested { request_id, .. } = e {
            control.answer_approval(request_id, true);
        }
    });
    assert!(
        matches!(result, ContentBlock::ToolResult { is_error: None, .. }),
        "{result:?}"
    );
    assert_eq!(
        git_stdout(root.path(), &["show", "--pretty=", "--name-only", "HEAD"]).trim(),
        "requested.txt"
    );
    assert!(git_stdout(root.path(), &["status", "--porcelain"]).contains("?? unrelated.txt"));
    std::fs::write(root.path().join("requested.txt"), "second\n").unwrap();
    git_stdout(root.path(), &["add", "requested.txt"]);
    let head = git_stdout(root.path(), &["rev-parse", "HEAD"]);
    let result = commit(&mut |e| {
        if let AgentProgress::ApprovalRequested { request_id, .. } = e {
            std::fs::write(
                root.path().join("requested.txt"),
                "changed during approval\n",
            )
            .unwrap();
            git_stdout(root.path(), &["add", "requested.txt"]);
            control.answer_approval(request_id, true);
        }
    });
    assert!(matches!(
        result,
        ContentBlock::ToolResult {
            is_error: Some(true),
            ..
        }
    ));
    assert_eq!(git_stdout(root.path(), &["rev-parse", "HEAD"]), head);
}

#[test]
fn declined_and_cancelled_approval_cannot_run_commands() {
    let root = tempfile::tempdir().unwrap();
    for cancel in [false, true] {
        let control = TaskControl::default();
        control.enable_interactive_approval();
        let result = crate::tools::run_tool_call_controlled(
            root.path(),
            "danger",
            "run_command",
            &json!({"command":["touch","bad"]}),
            &control,
            &mut |e| {
                if let AgentProgress::ApprovalRequested { request_id, .. } = e {
                    assert!(!control.answer_approval(request_id + 1, true));
                    if cancel {
                        control.cancel();
                    } else {
                        assert!(control.answer_approval(request_id, false));
                        assert!(!control.answer_approval(request_id, true));
                    }
                }
            },
        );
        assert!(matches!(
            result,
            ContentBlock::ToolResult {
                is_error: Some(true),
                ..
            }
        ));
        assert!(!root.path().join("bad").exists());
    }
}

#[tokio::test]
async fn round_limit_keeps_all_completed_results() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("f"), "hello").unwrap();
    let replies = (0..MAX_TOOL_ROUNDS)
        .map(|_| Ok(response(&[("read_file", json!({"path":"f"}))])))
        .collect();
    let mut agent = service(root.path(), replies);
    assert!(matches!(
        agent.submit("read".into()).await,
        Err(AgentError::ToolRoundsExhausted)
    ));
    drop(agent);
    let mut restarted = service(root.path(), vec![]);
    restarted.restore_state().unwrap();
    assert_eq!(restarted.messages.len(), 1 + 2 * MAX_TOOL_ROUNDS);
}

#[tokio::test]
async fn failed_result_save_recovers_unknown_without_repeating_edit() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = service(
        root.path(),
        vec![Ok(response(&[(
            "apply_patch",
            json!({"patch":"*** Begin Patch\n*** Add File: created\n+done\n*** End Patch"}),
        )]))],
    );
    let state = agent.conversation_path();
    let saved = state.with_extension("saved");
    let result=agent.submit_with_progress("Create a file".into(),|e|{
        if matches!(e,AgentProgress::ToolCallFinished{ref name,is_error:false} if name=="apply_patch") {
            std::fs::rename(&state,&saved).unwrap();std::fs::create_dir(&state).unwrap();
        }
    }).await;
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("outcome may be unknown")
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("created")).unwrap(),
        "done"
    );
    drop(agent);
    std::fs::remove_dir(&state).unwrap();
    std::fs::rename(&saved, &state).unwrap();
    let mut restarted = service(root.path(), vec![]);
    restarted.restore_state().unwrap();
    assert!(
        serde_json::to_string(&restarted.messages)
            .unwrap()
            .contains("Outcome unknown")
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("created")).unwrap(),
        "done"
    );
}

#[test]
fn failed_execution_intent_after_approval_has_no_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let control = TaskControl::default();
    control.enable_interactive_approval();
    let result = crate::tools::run_tool_call_checkpointed(
        root.path(),
        "call",
        "run_command",
        &json!({"command":["touch","must-not-exist"]}),
        &control,
        &crate::settings::Settings::default(),
        &mut |e| {
            if let AgentProgress::ApprovalRequested { request_id, .. } = e {
                control.answer_approval(request_id, true);
            }
        },
        &mut |_| Err(std::io::Error::other("injected disk failure")),
    );
    assert!(matches!(
        result,
        ContentBlock::ToolResult {
            is_error: Some(true),
            ..
        }
    ));
    assert!(!root.path().join("must-not-exist").exists());
}

#[test]
fn lock_child_probe() {
    let Some(root) = std::env::var_os("SOCIUM_TEST_LOCK_ROOT") else {
        return;
    };
    let err = crate::agent::state::lock_workspace(std::path::Path::new(&root)).unwrap_err();
    let owner = std::env::var("SOCIUM_TEST_LOCK_OWNER_PID").unwrap();
    assert!(
        err.to_string()
            .contains(&format!("in use by another socium (PID {owner})")),
        "{err}"
    );
}

#[test]
fn session_lock_rejects_a_second_process() {
    let root = tempfile::tempdir().unwrap();
    let _owner = crate::agent::state::lock_workspace(root.path()).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "reliability_tests::lock_child_probe"])
        .env("SOCIUM_TEST_LOCK_ROOT", root.path())
        .env("SOCIUM_TEST_LOCK_OWNER_PID", std::process::id().to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn activity_records_rounds_tools_and_turn_outcome_in_order() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("notes.txt"), "first line\nsecond\n").unwrap();
    let mut agent = service(
        root.path(),
        vec![
            Ok(response(&[
                ("read_file", json!({"path":"notes.txt"})),
                ("read_file", json!({"path":"missing.txt"})),
            ])),
            Ok(text_provider_response("done", None)),
        ],
    );
    let mut activity = Vec::new();
    agent
        .submit_with_progress("read it".into(), |event| {
            if let AgentProgress::Activity(event) = event {
                activity.push(event);
            }
        })
        .await
        .unwrap();

    let turn_id = activity[0].turn_id;
    assert!(activity.iter().all(|event| event.turn_id == turn_id));
    assert!(activity.windows(2).all(|w| w[0].at_ms <= w[1].at_ms));
    let shape = activity
        .iter()
        .map(|event| match &event.kind {
            ActivityKind::TurnStarted => format!("{} turn", event.round),
            ActivityKind::RoundStarted { .. } => format!("{} round", event.round),
            ActivityKind::RoundFinished { stop_reason, .. } => {
                format!("{} round done {stop_reason:?}", event.round)
            }
            ActivityKind::ToolStarted {
                name,
                input_summary,
                ..
            } => format!("{} {name} {input_summary}", event.round),
            ActivityKind::ToolFinished {
                is_error, summary, ..
            } => format!(
                "{} done error={is_error} {}",
                event.round,
                !summary.is_empty()
            ),
            ActivityKind::TurnFinished { ok, .. } => format!("{} turn ok={ok}", event.round),
            other => format!("unexpected {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shape,
        [
            "0 turn",
            "1 round",
            "1 round done None",
            "1 read_file notes.txt",
            "1 done error=false true",
            "1 read_file missing.txt",
            "1 done error=true true",
            "2 round",
            "2 round done None",
            "0 turn ok=true",
        ]
    );
}

#[tokio::test]
async fn activity_is_saved_replayed_after_restart_and_left_behind_by_clear() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = service(
        root.path(),
        vec![Ok(text_provider_response("first answer", None))],
    );
    let mut live = Vec::new();
    agent
        .submit_with_progress("hello".into(), |event| {
            if let AgentProgress::Activity(event) = event {
                live.push(event);
            }
        })
        .await
        .unwrap();
    drop(agent);

    let mut restarted = service(root.path(), vec![]);
    restarted.restore_state().unwrap();
    assert_eq!(restarted.recent_activity(), live);
    assert!(
        !live
            .iter()
            .any(|e| matches!(e.kind, ActivityKind::Warning { .. }))
    );

    let previous = restarted.session_id().to_string();
    restarted.clear().unwrap();
    assert_ne!(restarted.session_id(), previous);
    assert!(restarted.recent_activity().is_empty());
    restarted.session_id = previous;
    assert_eq!(restarted.recent_activity(), live);
}

fn open_session(root: &std::path::Path, start: SessionStart) -> Result<AgentService, AgentError> {
    AgentService::open(
        root.into(),
        start,
        crate::models::ModelRegistry::builtin(),
        Arc::new(|key| (key == "ANTHROPIC_API_KEY").then(|| "test".to_string())),
        None,
        reqwest::Client::new(),
    )
}

/// A saved session holding one user prompt.
fn saved_session(root: &std::path::Path, prompt: &str) -> String {
    let mut agent = open_session(root, SessionStart::New).unwrap();
    agent.messages = vec![Message::user_text(prompt.into())];
    agent.persist_state().unwrap();
    agent.session_id().to_string()
}

fn prompts(agent: &AgentService) -> Vec<String> {
    agent
        .transcript_entries()
        .into_iter()
        .map(|entry| entry.text)
        .collect()
}

#[test]
fn a_plain_start_opens_a_new_session_and_saves_nothing_until_it_has_messages() {
    let root = tempfile::tempdir().unwrap();
    let first = saved_session(root.path(), "first");

    let agent = open_session(root.path(), SessionStart::New).unwrap();
    assert_ne!(agent.session_id(), first);
    assert!(!agent.has_saved_messages());
    assert!(prompts(&agent).is_empty());
    assert!(agent.startup_warnings().is_empty());
    let id = agent.session_id().to_string();
    drop(agent);
    assert!(!root.path().join(".socium/sessions").join(&id).exists());
    assert_eq!(
        list_sessions(root.path())
            .unwrap()
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        [first.as_str()]
    );
}

#[test]
fn continue_opens_the_most_recently_saved_session() {
    let root = tempfile::tempdir().unwrap();
    saved_session(root.path(), "older");
    std::thread::sleep(Duration::from_millis(20));
    let newer = saved_session(root.path(), "newer");

    let agent = open_session(root.path(), SessionStart::Continue).unwrap();
    assert_eq!(agent.session_id(), newer);
    assert_eq!(prompts(&agent), ["newer"]);
    assert!(agent.startup_warnings().is_empty());
}

#[test]
fn continue_refuses_an_unreadable_latest_session_instead_of_skipping_it() {
    let root = tempfile::tempdir().unwrap();
    saved_session(root.path(), "older");
    std::thread::sleep(Duration::from_millis(20));
    let broken = saved_session(root.path(), "newer");
    let path = root
        .path()
        .join(".socium/sessions")
        .join(&broken)
        .join("conversation.json");
    std::fs::write(&path, "{broken").unwrap();

    let err = open_session(root.path(), SessionStart::Continue)
        .err()
        .unwrap();
    assert!(err.to_string().contains("Invalid JSON"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
}

#[test]
fn continue_without_a_saved_session_starts_a_new_one_and_says_so() {
    let root = tempfile::tempdir().unwrap();
    let agent = open_session(root.path(), SessionStart::Continue).unwrap();
    assert!(!agent.has_saved_messages());
    assert!(agent.startup_warnings()[0].contains("No saved session to continue"));
}

#[test]
fn resume_opens_the_named_session_and_refuses_an_unknown_one() {
    let root = tempfile::tempdir().unwrap();
    let older = saved_session(root.path(), "older");
    std::thread::sleep(Duration::from_millis(20));
    saved_session(root.path(), "newer");

    let agent = open_session(root.path(), SessionStart::Resume(older.clone())).unwrap();
    assert_eq!(agent.session_id(), older);
    assert_eq!(prompts(&agent), ["older"]);
    drop(agent);

    let err = open_session(root.path(), SessionStart::Resume("1999".into()))
        .err()
        .unwrap();
    assert!(err.to_string().contains("No saved session '1999'"), "{err}");
}

#[test]
fn the_pre_session_conversation_is_moved_and_continued() {
    let root = tempfile::tempdir().unwrap();
    let legacy = crate::workspace_state_path(root.path(), "conversation.json").unwrap();
    std::fs::write(
        &legacy,
        r#"{"schema_version":1,"messages":[{"role":"user","content":"from before"}]}"#,
    )
    .unwrap();
    std::fs::write(root.path().join(".socium/activity.jsonl"), "").unwrap();

    let agent = open_session(root.path(), SessionStart::Continue).unwrap();
    assert_eq!(prompts(&agent), ["from before"]);
    assert!(!legacy.exists());
    assert!(!root.path().join(".socium/activity.jsonl").exists());
    assert!(
        root.path()
            .join(".socium/sessions")
            .join(agent.session_id())
            .join("activity.jsonl")
            .is_file()
    );
}

#[test]
fn clear_keeps_the_previous_session_resumable() {
    let root = tempfile::tempdir().unwrap();
    let first = saved_session(root.path(), "keep me");
    let mut agent = open_session(root.path(), SessionStart::Resume(first.clone())).unwrap();

    agent.clear().unwrap();
    assert_ne!(agent.session_id(), first);
    assert!(prompts(&agent).is_empty());
    drop(agent);

    let resumed = open_session(root.path(), SessionStart::Resume(first)).unwrap();
    assert_eq!(prompts(&resumed), ["keep me"]);
}

#[test]
fn a_new_session_starts_on_the_model_chosen_in_the_last_one() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = open_session(root.path(), SessionStart::New).unwrap();
    agent.switch_model("sonnet").unwrap();
    drop(agent);

    let agent = open_session(root.path(), SessionStart::New).unwrap();
    assert_eq!(agent.active_model().alias, "sonnet");
}

#[test]
fn resume_switches_to_another_saved_session_and_lists_the_others() {
    let root = tempfile::tempdir().unwrap();
    let first = saved_session(root.path(), "first");
    std::thread::sleep(Duration::from_millis(20));
    let second = saved_session(root.path(), "second");
    let mut agent = open_session(root.path(), SessionStart::Resume(second.clone())).unwrap();
    assert_eq!(
        agent
            .list_sessions()
            .unwrap()
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        [first.as_str()]
    );

    let switch = agent.resume_session(&first).unwrap();
    assert!(switch.warnings.is_empty(), "{:?}", switch.warnings);
    assert!(!switch.needs_compaction);
    assert_eq!(agent.session_id(), first);
    assert_eq!(prompts(&agent), ["first"]);
    assert_eq!(
        agent
            .list_sessions()
            .unwrap()
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        [second.as_str()]
    );
}

#[test]
fn resume_refuses_the_open_or_an_unknown_session_and_keeps_the_current_one() {
    let root = tempfile::tempdir().unwrap();
    let id = saved_session(root.path(), "only");
    let mut agent = open_session(root.path(), SessionStart::Resume(id.clone())).unwrap();

    let err = agent.resume_session(&id).err().unwrap();
    assert!(err.to_string().contains("already open"), "{err}");
    let err = agent.resume_session("1999").err().unwrap();
    assert!(err.to_string().contains("No saved session"), "{err}");
    assert_eq!(agent.session_id(), id);
    assert_eq!(prompts(&agent), ["only"]);
}

#[test]
fn resume_applies_the_sessions_model_or_stays_and_drops_thinking() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = open_session(root.path(), SessionStart::New).unwrap();
    agent.switch_model("sonnet").unwrap();
    agent.messages = vec![Message::user_text("on sonnet".into())];
    agent.persist_state().unwrap();
    let on_sonnet = agent.session_id().to_string();
    drop(agent);

    // A session saved on a model that is no longer registered, with thinking.
    let gone = saved_session(root.path(), "on a removed model");
    let path = root
        .path()
        .join(".socium/sessions")
        .join(&gone)
        .join("conversation.json");
    let mut state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    state["active_model"] = json!("ds-flash");
    state["messages"] = json!([
        {"role": "user", "content": "on a removed model"},
        {"role": "assistant", "content": [
            {"type": "thinking", "thinking": "plan", "signature": "sig"},
            {"type": "text", "text": "answer"}
        ]}
    ]);
    std::fs::write(&path, state.to_string()).unwrap();

    let mut agent = open_session(root.path(), SessionStart::New).unwrap();
    agent.switch_model("opus").unwrap();
    let switch = agent.resume_session(&on_sonnet).unwrap();
    assert_eq!(switch.model.alias, "sonnet");
    assert_eq!(agent.active_model().alias, "sonnet");

    let switch = agent.resume_session(&gone).unwrap();
    assert_eq!(switch.model.alias, "sonnet");
    assert!(
        switch.warnings[0].contains("'ds-flash' is no longer registered; staying on 'sonnet'"),
        "{:?}",
        switch.warnings
    );
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(saved.get("active_model").is_none());
    assert!(!saved.to_string().contains("\"thinking\""));
}

#[test]
fn a_named_session_is_listed_and_resumed_by_its_name() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = open_session(root.path(), SessionStart::New).unwrap();
    agent.messages = vec![Message::user_text("named".into())];
    agent.rename_session("  readme fix  ").unwrap();
    assert_eq!(agent.session_name(), Some("readme fix"));
    let id = agent.session_id().to_string();
    for bad in ["", "   ", "two\nlines", &"x".repeat(81)] {
        assert!(agent.rename_session(bad).is_err(), "{bad:?}");
    }
    assert_eq!(agent.session_name(), Some("readme fix"));
    drop(agent);

    assert_eq!(
        list_sessions(root.path()).unwrap()[0].name.as_deref(),
        Some("readme fix")
    );
    let agent = open_session(root.path(), SessionStart::Resume("readme fix".into())).unwrap();
    assert_eq!(agent.session_id(), id);
    assert_eq!(agent.session_name(), Some("readme fix"));
    drop(agent);

    // The same name on two sessions is ambiguous.
    let mut other = open_session(root.path(), SessionStart::New).unwrap();
    other.messages = vec![Message::user_text("other".into())];
    other.rename_session("readme fix").unwrap();
    drop(other);
    let err = open_session(root.path(), SessionStart::Resume("readme fix".into()))
        .err()
        .unwrap();
    assert!(err.to_string().contains("ambiguous"), "{err}");
}

#[test]
fn clear_starts_a_session_without_the_previous_name() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = open_session(root.path(), SessionStart::New).unwrap();
    agent.rename_session("first").unwrap();
    agent.clear().unwrap();
    assert_eq!(agent.session_name(), None);
}

#[test]
fn delete_removes_another_session_and_refuses_the_open_or_unknown_one() {
    let root = tempfile::tempdir().unwrap();
    let other = saved_session(root.path(), "delete me");
    let open = saved_session(root.path(), "keep me");
    let mut agent = open_session(root.path(), SessionStart::Resume(open.clone())).unwrap();

    let err = agent.delete_session(&open).unwrap_err();
    assert!(err.to_string().contains("cannot be deleted"), "{err}");
    for unknown in ["20000101-000000-0000", "../..", "", "sessions"] {
        assert_eq!(
            agent.delete_session(unknown).unwrap_err().kind(),
            std::io::ErrorKind::NotFound,
            "{unknown}"
        );
    }
    agent.delete_session(&other).unwrap();
    assert!(!root.path().join(".socium/sessions").join(&other).exists());
    assert!(agent.list_sessions().unwrap().is_empty());
    assert_eq!(prompts(&agent), ["keep me"]);
}

#[cfg(unix)]
#[test]
fn delete_refuses_a_session_path_that_is_a_symlink() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("keep"), "x").unwrap();
    let mut agent = open_session(root.path(), SessionStart::New).unwrap();
    std::fs::create_dir_all(root.path().join(".socium/sessions")).unwrap();
    let link = root.path().join(".socium/sessions/20000101-000000-0000");
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();

    assert!(agent.delete_session("20000101-000000-0000").is_err());
    assert!(outside.path().join("keep").exists());
}

fn run_with_settings(
    root: &std::path::Path,
    settings: &crate::settings::Settings,
    control: &TaskControl,
    name: &str,
    input: serde_json::Value,
    on_progress: &mut dyn FnMut(AgentProgress),
) -> (bool, String) {
    match crate::tools::run_tool_call_checkpointed(
        root,
        "call",
        name,
        &input,
        control,
        settings,
        &mut |event| on_progress(event),
        &mut |_| Ok(()),
    ) {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => (is_error != Some(true), content),
        other => panic!("{other:?}"),
    }
}

#[test]
fn auto_approved_commands_run_unasked_without_secrets_or_host_run_files() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".env"), "TOKEN=fake-secret\n").unwrap();
    std::fs::write(root.path().join("build.rs"), "fn main() {}\n").unwrap();
    let (settings, _) =
        crate::settings::Settings::from_toml("[approval]\ncommands = \"auto\"", None).unwrap();
    let control = TaskControl::default();
    let mut asked = false;
    let (_, output) = run_with_settings(
        root.path(),
        &settings,
        &control,
        "run_command",
        json!({"command": ["sh", "-c", "cat .env; echo changed > build.rs; echo ok > out.txt"]}),
        &mut |event| asked |= matches!(event, AgentProgress::ApprovalRequested { .. }),
    );
    assert!(!asked);
    assert!(!output.contains("fake-secret"), "{output}");
    assert_eq!(
        std::fs::read_to_string(root.path().join("build.rs")).unwrap(),
        "fn main() {}\n"
    );
    assert!(root.path().join("out.txt").exists());
}

#[test]
fn a_session_grant_stops_asking_for_the_same_command_prefix() {
    let root = tempfile::tempdir().unwrap();
    let settings = crate::settings::Settings::default();
    let control = TaskControl::default();
    control.enable_interactive_approval();
    let mut asked = 0;
    for _ in 0..2 {
        let (ok, _) = run_with_settings(
            root.path(),
            &settings,
            &control,
            "run_command",
            json!({"command": ["touch", "marker"]}),
            &mut |event| {
                if let AgentProgress::ApprovalRequested {
                    request_id,
                    session_option,
                    ..
                } = event
                {
                    asked += 1;
                    assert!(session_option.unwrap().contains("`touch marker`"));
                    control.answer(request_id, crate::ApprovalAnswer::AllowForSession);
                }
            },
        );
        assert!(ok);
    }
    assert_eq!(asked, 1);
}

#[test]
fn without_a_ui_a_call_that_needs_approval_says_how_to_allow_it() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".env"), "TOKEN=fake-secret\n").unwrap();
    let settings = crate::settings::Settings::default();
    let control = TaskControl::default();
    for (name, input) in [
        ("run_command", json!({"command": ["touch", "marker"]})),
        ("read_file", json!({"path": ".env"})),
    ] {
        let (ok, output) =
            run_with_settings(root.path(), &settings, &control, name, input, &mut |_| {});
        assert!(!ok);
        assert!(output.contains("config.toml"), "{output}");
        assert!(!output.contains("fake-secret"));
    }
    assert!(!root.path().join("marker").exists());
}
