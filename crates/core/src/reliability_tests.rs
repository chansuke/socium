//! Offline acceptance scenarios for everyday work, not provider-quality evals.
#![expect(
    clippy::disallowed_methods,
    reason = "scenarios read the test environment and give a racing peer a head start"
)]
use super::*;
use crate::conversation::{AgentMessageResponse, ContentBlock, MessageContent};
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
            ..Default::default()
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
        crate::test_support::http_client(),
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
        _: &crate::conversation::AgentMessageRequest<'_>,
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
        crate::test_support::http_client(),
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
                    &json!({"patch":patch,"dry_run":dry_run}),
                    None
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
        "done\n"
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
        "done\n"
    );
}

#[test]
fn failed_execution_intent_after_approval_has_no_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let control = TaskControl::default();
    control.enable_interactive_approval();
    let result = crate::tools::run_tool_call_checkpointed(
        crate::tools::ToolCall {
            workspace_root: root.path(),
            tool_use_id: "call",
            name: "run_command",
            input: &json!({"command":["touch","must-not-exist"]}),
            control: &control,
            settings: &crate::settings::Settings::default(),
            edit_origin: None,
        },
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
        crate::test_support::http_client(),
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
fn continuing_the_newest_session_leaves_its_file_alone() {
    let root = tempfile::tempdir().unwrap();
    let id = saved_session(root.path(), "only");
    let path = root
        .path()
        .join(".socium/sessions")
        .join(&id)
        .join("conversation.json");
    let saved_at = std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(saved_at)
        .unwrap();

    drop(open_session(root.path(), SessionStart::Continue).unwrap());
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        saved_at
    );
}

#[test]
fn a_resumed_older_session_is_the_one_continue_opens_next() {
    let root = tempfile::tempdir().unwrap();
    let older = saved_session(root.path(), "older");
    std::thread::sleep(Duration::from_millis(20));
    saved_session(root.path(), "newer");

    drop(open_session(root.path(), SessionStart::Resume(older.clone())).unwrap());
    let agent = open_session(root.path(), SessionStart::Continue).unwrap();
    assert_eq!(agent.session_id(), older);
}

#[test]
fn continuing_saves_the_repair_of_an_interrupted_tool_call() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = open_session(root.path(), SessionStart::New).unwrap();
    agent.messages = vec![
        Message::user_text("list the files".into()),
        Message::assistant_blocks(vec![ContentBlock::ToolUse {
            id: "call-1".into(),
            name: "list_files".into(),
            input: json!({"path": "."}),
        }]),
    ];
    agent.persist_state().unwrap();
    let path = agent.conversation_path();
    drop(agent);

    drop(open_session(root.path(), SessionStart::Continue).unwrap());
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(
        saved.contains("Interrupted before result was saved"),
        "{saved}"
    );
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
fn resume_applies_the_sessions_model_or_stays_and_keeps_thinking_unsent() {
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
    // The saved history keeps the block; it has no recorded origin, so no
    // provider is sent it.
    assert!(saved.to_string().contains("\"thinking\""));
    let sent = crate::conversation::readable_thinking(&agent.messages, &|origin| {
        agent.provider.reads_thinking_from(origin)
    })
    .unwrap();
    assert!(
        !serde_json::to_string(&sent)
            .unwrap()
            .contains("\"thinking\"")
    );
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
        crate::tools::ToolCall {
            workspace_root: root,
            tool_use_id: "call",
            name,
            input: &input,
            control,
            settings,
            edit_origin: None,
        },
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

fn status(code: u16) -> Result<ProviderResponse, ProviderError> {
    Err(ProviderError::Status {
        status: code,
        retry_after: Some(Duration::ZERO),
        message: format!("test API error {code}"),
    })
}

#[tokio::test]
async fn overload_and_rate_limits_are_retried_before_any_output() {
    let root = tempfile::tempdir().unwrap();
    let provider = ScriptedProvider::new(vec![
        status(529),
        status(429),
        Ok(text_provider_response("third time", None)),
    ]);
    let requests = provider.recorded_requests.clone();
    let mut agent = AgentService::with_provider(
        root.path().into(),
        Box::new(provider),
        "test".into(),
        1000,
        crate::test_support::http_client(),
    );
    let mut warnings = Vec::new();
    let reply = agent
        .submit_with_progress("hi".into(), |event| {
            if let AgentProgress::Activity(ActivityEvent {
                kind: ActivityKind::Warning { message },
                ..
            }) = event
            {
                warnings.push(message);
            }
        })
        .await
        .unwrap();
    assert_eq!(reply.text, "third time");
    assert_eq!(requests.lock().unwrap().len(), 3);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("attempt 2 of 4"));
}

#[tokio::test]
async fn invalid_requests_fail_at_once_and_retries_stop_after_four_attempts() {
    let root = tempfile::tempdir().unwrap();
    for (responses, attempts) in [
        (vec![status(400)], 1),
        (vec![status(503), status(503), status(503), status(503)], 4),
    ] {
        let provider = ScriptedProvider::new(responses);
        let requests = provider.recorded_requests.clone();
        let mut agent = AgentService::with_provider(
            root.path().into(),
            Box::new(provider),
            "test".into(),
            1000,
            crate::test_support::http_client(),
        );
        assert!(agent.submit("hi".into()).await.is_err());
        assert_eq!(requests.lock().unwrap().len(), attempts);
        drop(agent);
    }
}

#[tokio::test]
async fn a_call_with_unparsable_arguments_is_reported_and_never_runs() {
    let root = tempfile::tempdir().unwrap();
    let mut first = response(&[("apply_patch", json!({}))]);
    first.message.invalid_tool_inputs = vec![("call-0".into(), "EOF while parsing".into())];
    let provider = ScriptedProvider::new(vec![
        Ok(first),
        Ok(text_provider_response("understood", None)),
    ]);
    let requests = provider.recorded_requests.clone();
    let mut agent = AgentService::with_provider(
        root.path().into(),
        Box::new(provider),
        "test".into(),
        1000,
        crate::test_support::http_client(),
    );
    assert_eq!(
        agent.submit("edit".into()).await.unwrap().text,
        "understood"
    );
    let second = requests.lock().unwrap()[1].to_string();
    assert!(second.contains("were not valid JSON"), "{second}");
}

#[tokio::test]
async fn a_reply_cut_at_the_output_limit_ends_the_turn_with_a_notice() {
    let root = tempfile::tempdir().unwrap();
    let mut cut = text_provider_response("partial answer", None);
    cut.message.stop_reason = Some("max_tokens".into());
    let mut agent = AgentService::with_provider(
        root.path().into(),
        Box::new(ScriptedProvider::new(vec![Ok(cut)])),
        "test".into(),
        1000,
        crate::test_support::http_client(),
    );
    let reply = agent.submit("long".into()).await.unwrap();
    assert_eq!(reply.text, "partial answer");
    assert!(
        reply.notices[0].contains("output limit"),
        "{:?}",
        reply.notices
    );
}

/// Writes `count` files of about `bytes` each and returns a response that
/// reads them all in one round.
fn large_reads(root: &std::path::Path, count: usize, bytes: usize) -> ProviderResponse {
    let line = "let value = compute(input, 1234567890); // filler line\n";
    let names = (0..count).map(|i| format!("f{i}.rs")).collect::<Vec<_>>();
    for name in &names {
        std::fs::write(root.join(name), line.repeat(bytes / line.len())).unwrap();
    }
    let calls = names
        .iter()
        .map(|name| ("read_file", json!({ "path": name })))
        .collect::<Vec<_>>();
    response(&calls)
}

fn scripted_agent(
    root: &std::path::Path,
    responses: Vec<Result<ProviderResponse, ProviderError>>,
    max_tokens: u32,
) -> (AgentService, Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
    let provider = ScriptedProvider::new(responses);
    let requests = provider.recorded_requests.clone();
    let agent = AgentService::with_provider(
        root.into(),
        Box::new(provider),
        "test".into(),
        max_tokens,
        crate::test_support::http_client(),
    );
    (agent, requests)
}

#[tokio::test]
async fn large_parallel_reads_reach_the_model_instead_of_stranding_the_session() {
    // Review R3: three ~199 KB reads in a 200k window used to fail the
    // turn, then the next seven messages.
    let root = tempfile::tempdir().unwrap();
    let reads = large_reads(root.path(), 3, 199_000);
    let (mut agent, requests) = scripted_agent(
        root.path(),
        vec![Ok(reads), Ok(text_provider_response("compared", None))],
        64_000,
    );
    for turn in 0..5 {
        agent
            .messages
            .push(Message::user_text(format!("earlier question {turn}")));
        agent
            .messages
            .push(Message::assistant_blocks(vec![ContentBlock::Text {
                text: format!("earlier answer {turn}"),
            }]));
    }
    let reply = agent.submit("Compare the files".into()).await.unwrap();
    assert_eq!(reply.text, "compared");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].to_string().contains("compute(input"));
}

#[tokio::test]
async fn results_that_overflow_the_window_mid_turn_are_cut_and_saved_as_sent() {
    let root = tempfile::tempdir().unwrap();
    let reads = large_reads(root.path(), 3, 90_000);
    let (mut agent, requests) = scripted_agent(
        root.path(),
        vec![Ok(reads), Ok(text_provider_response("done", None))],
        10_000,
    );
    // 60k window - 10k output = 50k tokens, about 175k characters.
    agent.active_model.context_window = Some(60_000);
    let mut warnings = Vec::new();
    agent
        .submit_with_progress("read them".into(), |event| {
            if let AgentProgress::Activity(ActivityEvent {
                kind: ActivityKind::Warning { message },
                ..
            }) = event
            {
                warnings.push(message);
            }
        })
        .await
        .unwrap();
    assert!(
        warnings.iter().any(|w| w.contains("cut to fit")),
        "{warnings:?}"
    );
    let sent = requests.lock().unwrap()[1].to_string();
    assert!(sent.contains("omitted to fit the context window"));
    let saved = serde_json::to_string(&agent.messages).unwrap();
    assert!(saved.contains("omitted to fit the context window"));
}

#[tokio::test]
async fn a_window_that_fills_mid_turn_is_compacted_at_once() {
    let root = tempfile::tempdir().unwrap();
    // Twenty results cannot shrink below 2,000 characters each.
    let reads = large_reads(root.path(), 20, 20_000);
    let (mut agent, requests) = scripted_agent(
        root.path(),
        vec![
            Ok(reads),
            Ok(text_provider_response(
                "1. User request: earlier work",
                None,
            )),
            Ok(text_provider_response("continuing", None)),
        ],
        10_000,
    );
    agent.active_model.context_window = Some(60_000);
    // Earlier turns hold most of the 50k-token budget, about 210k
    // characters at the estimate's 4.2 per token for this model name.
    for turn in 0..2 {
        agent
            .messages
            .push(Message::user_text(format!("question {turn}")));
        agent
            .messages
            .push(Message::assistant_blocks(vec![ContentBlock::Text {
                text: "x".repeat(90_000),
            }]));
    }
    let err = agent
        .submit("read all".into())
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("filled during the tool loop"), "{err}");
    assert!(err.contains(" tokens). The history was compacted"), "{err}");
    assert!(agent.summary_state.summary.is_some());
    assert_eq!(requests.lock().unwrap().len(), 2);
    let reply = agent.submit("continue".into()).await.unwrap();
    assert_eq!(reply.text, "continuing");
}

#[tokio::test]
async fn the_first_request_survives_compaction_word_for_word() {
    let root = tempfile::tempdir().unwrap();
    let request = format!(
        "{} Finally, keep the public API unchanged.",
        "Context for the task. ".repeat(80)
    );
    let (mut agent, requests) = scripted_agent(
        root.path(),
        vec![
            Err(ProviderError::Api("compaction model down".into())),
            Ok(text_provider_response("next", None)),
        ],
        1_000,
    );
    agent.messages.push(Message::user_text(request.clone()));
    for turn in 0..20 {
        agent
            .messages
            .push(Message::assistant_blocks(vec![ContentBlock::Text {
                text: format!("answer {turn}"),
            }]));
        agent
            .messages
            .push(Message::user_text(format!("follow-up {turn}")));
    }
    let report = agent.compact().await.unwrap();
    assert!(!report.used_llm);
    assert!(
        report
            .fallback_reason
            .as_deref()
            .unwrap()
            .contains("compaction model down"),
        "{:?}",
        report.fallback_reason
    );
    agent.submit("go on".into()).await.unwrap();
    let sent = requests.lock().unwrap()[1].to_string();
    assert!(sent.contains("keep the public API unchanged"), "{sent}");
    assert!(sent.contains("first request, word for word"));
}

async fn submit_turns(agent: &mut AgentService, prompts: &[&str]) {
    for prompt in prompts {
        agent.submit((*prompt).into()).await.unwrap();
    }
}

#[tokio::test]
async fn rewind_goes_back_to_a_prompt_and_drops_the_turns_from_it() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = service(
        root.path(),
        vec![
            Ok(text_provider_response("answer one", None)),
            Ok(text_provider_response("answer two", None)),
            Ok(text_provider_response("answer three", None)),
        ],
    );
    submit_turns(&mut agent, &["first", "second", "third"]).await;
    let points = agent.rewind_points();
    assert_eq!(
        points
            .iter()
            .map(|point| point.prompt.as_str())
            .collect::<Vec<_>>(),
        ["first", "second", "third"]
    );

    let rewound = agent
        .rewind(points[1].turn_id, RewindMode::Conversation)
        .unwrap();
    assert_eq!(rewound.prompt, "second");
    assert_eq!(rewound.warning, None);
    assert!(rewound.restored_files.is_empty());
    assert_eq!(prompts(&agent), ["first", "answer one"]);
    assert_eq!(agent.rewind_points().len(), 1);

    drop(agent);

    let mut agent = service(
        root.path(),
        vec![Ok(text_provider_response("answer again", None))],
    );
    agent.restore_state().unwrap();
    assert_eq!(prompts(&agent), ["first", "answer one"]);
    agent.submit("second, edited".into()).await.unwrap();
    assert_eq!(
        prompts(&agent),
        ["first", "answer one", "second, edited", "answer again"]
    );
    let turns = agent
        .recent_activity()
        .into_iter()
        .filter(|event| matches!(event.kind, ActivityKind::TurnStarted))
        .map(|event| event.turn_id)
        .collect::<Vec<_>>();
    assert_eq!(turns, [0, 2]);
}

#[tokio::test]
async fn rewind_refuses_a_message_that_is_not_a_prompt() {
    let root = tempfile::tempdir().unwrap();
    let mut agent = service(
        root.path(),
        vec![Ok(text_provider_response("answer", None))],
    );
    submit_turns(&mut agent, &["only"]).await;
    std::fs::write(root.path().join("a.txt"), "one\n").unwrap();
    crate::tools::apply_patch::apply_patch_from_workspace(
        root.path(),
        &json!({"patch": "*** Begin Patch\n*** Update File: a.txt\n@@\n-one\n+two\n*** End Patch"}),
        None,
    )
    .unwrap();
    agent.undo(None).unwrap();
    let before = prompts(&agent);

    for turn_id in [1, 2, 9] {
        let err = agent
            .rewind(turn_id, RewindMode::ConversationAndFiles)
            .err()
            .unwrap();
        assert!(err.to_string().contains("No prompt"), "{err}");
    }
    assert_eq!(prompts(&agent), before);
    assert_eq!(agent.rewind_points().len(), 1);
}

#[tokio::test]
async fn rewinding_into_the_summarized_history_drops_the_summary() {
    let root = tempfile::tempdir().unwrap();
    let mut responses = (0..10)
        .map(|turn| Ok(text_provider_response(&format!("answer {turn}"), None)))
        .collect::<Vec<_>>();
    responses.push(Err(ProviderError::Api("compaction model down".into())));
    let mut agent = service(root.path(), responses);
    let prompts = (0..10)
        .map(|turn| format!("prompt {turn}"))
        .collect::<Vec<_>>();
    submit_turns(
        &mut agent,
        &prompts.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .await;
    let covered = agent.compact().await.unwrap().covered_messages;
    assert!(covered > 2);

    agent.rewind(16, RewindMode::Conversation).unwrap();
    let summary = agent.summary_state.summary.as_ref().unwrap();
    assert_eq!(summary.covered_messages, covered);
    assert_eq!(summary.thinking_valid_from, Some(16));

    agent.rewind(2, RewindMode::Conversation).unwrap();
    assert!(agent.summary_state.summary.is_none());
    assert_eq!(agent.messages.len(), 2);
}

fn patch_call(path: &str, from: &str, to: &str) -> ProviderResponse {
    response(&[(
        "apply_patch",
        json!({"patch": format!("*** Begin Patch\n*** Update File: {path}\n@@\n-{from}\n+{to}\n*** End Patch")}),
    )])
}

fn read(root: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(root.join(path)).unwrap()
}

#[tokio::test]
async fn rewind_restores_only_the_files_the_dropped_turns_edited() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "one\n").unwrap();
    std::fs::write(root.path().join("b.txt"), "one\n").unwrap();
    let mut agent = service(
        root.path(),
        vec![
            Ok(patch_call("a.txt", "one", "two")),
            Ok(text_provider_response("edited a", None)),
            Ok(patch_call("b.txt", "one", "two")),
            Ok(patch_call("b.txt", "two", "three")),
            Ok(text_provider_response("edited b twice", None)),
        ],
    );
    submit_turns(&mut agent, &["edit a", "edit b"]).await;
    assert_eq!(read(root.path(), "b.txt"), "three\n");
    let points = agent.rewind_points();
    assert_eq!(points[0].changed_files, ["a.txt", "b.txt"]);
    assert_eq!(points[1].changed_files, ["b.txt"]);

    let rewound = agent
        .rewind(points[1].turn_id, RewindMode::ConversationAndFiles)
        .unwrap();
    assert_eq!(rewound.prompt, "edit b");
    assert_eq!(rewound.restored_files, ["b.txt"]);
    assert_eq!(read(root.path(), "a.txt"), "two\n");
    assert_eq!(read(root.path(), "b.txt"), "one\n");
    assert_eq!(agent.messages.len(), points[1].turn_id);
    let points = agent.rewind_points();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].changed_files, ["a.txt"]);
}

#[tokio::test]
async fn rewind_with_files_refuses_a_file_changed_by_hand_and_changes_nothing() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "one\n").unwrap();
    let mut agent = service(
        root.path(),
        vec![
            Ok(patch_call("a.txt", "one", "two")),
            Ok(text_provider_response("edited a", None)),
        ],
    );
    submit_turns(&mut agent, &["edit a"]).await;
    std::fs::write(root.path().join("a.txt"), "human\n").unwrap();
    let before = prompts(&agent);

    for mode in [RewindMode::ConversationAndFiles, RewindMode::Files] {
        let err = agent.rewind(0, mode).err().unwrap();
        assert!(
            err.to_string().contains("changed externally: 'a.txt'"),
            "{err}"
        );
    }
    assert_eq!(read(root.path(), "a.txt"), "human\n");
    assert_eq!(prompts(&agent), before);
    assert_eq!(agent.rewind_points()[0].changed_files, ["a.txt"]);
}

#[tokio::test]
async fn a_conversation_only_rewind_keeps_the_dropped_edits_out_of_later_rewinds() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "one\n").unwrap();
    std::fs::write(root.path().join("b.txt"), "one\n").unwrap();
    let mut agent = service(
        root.path(),
        vec![
            Ok(patch_call("a.txt", "one", "two")),
            Ok(text_provider_response("edited a", None)),
            Ok(patch_call("b.txt", "one", "two")),
            Ok(text_provider_response("edited b", None)),
        ],
    );
    submit_turns(&mut agent, &["edit a"]).await;
    let rewound = agent.rewind(0, RewindMode::Conversation).unwrap();
    assert_eq!(rewound.warning, None);
    assert!(rewound.restored_files.is_empty());
    assert_eq!(read(root.path(), "a.txt"), "two\n");

    // The new turn starts at the same message as the dropped one.
    submit_turns(&mut agent, &["edit b"]).await;
    let points = agent.rewind_points();
    assert_eq!(points[0].turn_id, 0);
    assert_eq!(points[0].changed_files, ["b.txt"]);
    let rewound = agent.rewind(0, RewindMode::ConversationAndFiles).unwrap();
    assert_eq!(rewound.restored_files, ["b.txt"]);
    assert_eq!(read(root.path(), "b.txt"), "one\n");
    assert_eq!(read(root.path(), "a.txt"), "two\n");

    assert_eq!(agent.undo(None).unwrap(), ["a.txt"]);
    assert_eq!(read(root.path(), "a.txt"), "one\n");
}

#[tokio::test]
async fn a_files_only_rewind_keeps_the_conversation_and_tells_the_model() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "one\n").unwrap();
    let mut agent = service(
        root.path(),
        vec![
            Ok(patch_call("a.txt", "one", "two")),
            Ok(text_provider_response("edited a", None)),
        ],
    );
    submit_turns(&mut agent, &["edit a"]).await;
    let kept = agent.messages.len();

    let rewound = agent.rewind(0, RewindMode::Files).unwrap();
    assert_eq!(rewound.restored_files, ["a.txt"]);
    assert_eq!(read(root.path(), "a.txt"), "one\n");
    assert_eq!(agent.messages.len(), kept + 1);
    let note = serde_json::to_string(agent.messages.last().unwrap()).unwrap();
    assert!(
        note.contains(r#"reverted the files you changed since their prompt \"edit a\": a.txt"#),
        "{note}"
    );
    let points = agent.rewind_points();
    assert_eq!(points.len(), 1);
    assert!(points[0].changed_files.is_empty());

    let err = agent.rewind(0, RewindMode::Files).err().unwrap();
    assert!(err.to_string().contains("changed no files"), "{err}");
    assert_eq!(agent.messages.len(), kept + 1);
}

#[test]
fn undo_reverts_the_last_edit_and_tells_the_model() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "one\n").unwrap();
    let mut agent = open_session(root.path(), SessionStart::New).unwrap();
    crate::tools::apply_patch::apply_patch_from_workspace(
        root.path(),
        &json!({"patch": "*** Begin Patch\n*** Update File: a.txt\n@@\n-one\n+two\n*** End Patch"}),
        None,
    )
    .unwrap();
    assert_eq!(agent.undo(None).unwrap(), ["a.txt"]);
    assert_eq!(
        std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
        "one\n"
    );
    let note = serde_json::to_string(agent.messages.last().unwrap()).unwrap();
    assert!(note.contains("undid your last edit of a.txt"), "{note}");
    assert!(agent.undo(Some("a.txt")).is_err());
}

#[test]
fn a_protected_file_that_changes_during_approval_is_not_edited() {
    let root = tempfile::tempdir().unwrap();
    let build = root.path().join("build.rs");
    std::fs::write(&build, "fn main() {}\n// one\n").unwrap();
    let settings = crate::settings::Settings::default();
    let control = TaskControl::default();
    control.enable_interactive_approval();
    let (ok, output) = run_with_settings(
        root.path(),
        &settings,
        &control,
        crate::constants::TEXT_EDITOR_TOOL_NAME,
        json!({"command": "str_replace", "path": "build.rs", "old_str": "// one", "new_str": "// two"}),
        &mut |event| {
            if let AgentProgress::ApprovalRequested {
                request_id, input, ..
            } = event
            {
                assert!(input.contains("+// two"), "{input}");
                // Someone else edits the file before the answer.
                std::fs::write(&build, "fn main() { other(); }\n// one\n").unwrap();
                control.answer(request_id, crate::ApprovalAnswer::AllowOnce);
            }
        },
    );
    assert!(!ok);
    assert!(output.contains("changed during approval"), "{output}");
    assert_eq!(
        std::fs::read_to_string(&build).unwrap(),
        "fn main() { other(); }\n// one\n"
    );
}
