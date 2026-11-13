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
    assert!(
        agent
            .submit_with_progress("Update old to new and test".into(), |event| {
                if let AgentProgress::ApprovalRequested { request_id, .. } = event {
                    control.answer_approval(request_id, true);
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
    assert!(
        restarted
            .transcript_entries()
            .iter()
            .any(|e| e.text.contains("Exit code: 0"))
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
    let path = first.conversation_path.clone();
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
    std::fs::create_dir_all(&agent.conversation_path).unwrap();
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
    let state = agent.conversation_path.clone();
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
        &mut |e| {
            if let AgentProgress::ApprovalRequested { request_id, .. } = e {
                control.answer_approval(request_id, true);
            }
        },
        &mut || Err(std::io::Error::other("injected disk failure")),
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
    assert!(crate::agent::state::lock_workspace(std::path::Path::new(&root)).is_err());
}

#[test]
fn session_lock_rejects_a_second_process() {
    let root = tempfile::tempdir().unwrap();
    let _owner = crate::agent::state::lock_workspace(root.path()).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "reliability_tests::lock_child_probe"])
        .env("SOCIUM_TEST_LOCK_ROOT", root.path())
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
async fn activity_is_saved_replayed_after_restart_and_rotated_by_clear() {
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

    restarted.clear().unwrap();
    assert!(restarted.recent_activity().is_empty());
    assert!(root.path().join(".socium/activity.1.jsonl").is_file());
}
