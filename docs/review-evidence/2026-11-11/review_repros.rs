use crate::agent::context::prepare_context_messages;
use crate::agent::state::ConversationSummaryState;
use crate::agent::types::{AgentMessageResponse, ContentBlock, Message, Role};
use crate::providers::{ProviderKind, ProviderResponse};
use crate::providers::error::ProviderError;
use crate::test_support::{make_temp_dir, ScriptedProvider};
use crate::tools::{run_tool_call, default_tools};
use serde_json::json;
use std::fs;

fn output(result: ContentBlock) -> (String, bool) {
    match result {
        ContentBlock::ToolResult{content, is_error, ..} => (content, is_error.unwrap_or(false)),
        _ => panic!("expected tool result"),
    }
}

#[test]
fn review_dry_run_accepts_missing_update_and_unchecked_destination() {
    let w=make_temp_dir("review-preview");
    let patch="*** Begin Patch\n*** Update File: missing.txt\n@@\n-before\n+after\n*** End Patch";
    let (_,dry_failed)=output(run_tool_call(w.path(),"t","apply_patch",&json!({"patch":patch,"dry_run":true})));
    let (_,apply_failed)=output(run_tool_call(w.path(),"t","apply_patch",&json!({"patch":patch})));
    assert!(!dry_failed);
    assert!(apply_failed);
}

#[test]
fn review_git_commit_includes_unrelated_unstaged_files() {
    let w=make_temp_dir("review-git");
    crate::test_support::init_git_repo(w.path());
    fs::write(w.path().join("requested.txt"),"requested").unwrap();
    crate::test_support::run_git(w.path(),&["add","requested.txt"]);
    fs::write(w.path().join("unrelated.txt"),"unrelated").unwrap();
    let (text,failed)=output(run_tool_call(w.path(),"t","git_commit",&json!({"message":"requested change","confirmed":true})));
    assert!(!failed,"{text}");
    let changed=crate::test_support::git_stdout(w.path(),&["show","--format=","--name-only","HEAD"]);
    assert!(changed.contains("unrelated.txt"));
}

#[test]
fn review_model_supplied_approval_is_sufficient() {
    let w=make_temp_dir("review-approval");
    fs::write(w.path().join("disposable.txt"), "test").unwrap();
    let (_, failed)=output(run_tool_call(w.path(),"t","run_command",&json!({
        "command":["rm","disposable.txt"], "approval":"destructive"
    })));
    assert!(!failed);
    assert!(!w.path().join("disposable.txt").exists());
}

#[test]
fn review_safe_read_only_sed_can_write_without_approval() {
    let w=make_temp_dir("review-classifier");
    fs::write(w.path().join("file.txt"), "before\n").unwrap();
    let (text, failed)=output(run_tool_call(w.path(),"t","run_command",&json!({
        "command":["sed","-i","s/before/after/","file.txt"]
    })));
    assert!(!failed);
    assert!(text.contains("safe-read-only"));
    assert_eq!(fs::read_to_string(w.path().join("file.txt")).unwrap(),"after\n");
}

#[test]
fn review_command_reads_outside_workspace() {
    let w=make_temp_dir("review-command-root");
    let other=make_temp_dir("review-command-outside");
    let file=other.path().join("fixture.txt");
    fs::write(&file,"nonsecret-outside-fixture").unwrap();
    let (text, failed)=output(run_tool_call(w.path(),"t","run_command",&json!({
        "command":["cat",file.to_str().unwrap()]
    })));
    assert!(!failed);
    assert!(text.contains("nonsecret-outside-fixture"));
}

#[test]
fn review_pipe_output_causes_false_timeout() {
    let w=make_temp_dir("review-pipe");
    fs::write(w.path().join("large.txt"), "x".repeat(1024*1024)).unwrap();
    let (text, failed)=output(run_tool_call(w.path(),"t","run_command",&json!({
        "command":["cat","large.txt"],"timeout_ms":150
    })));
    assert!(failed);
    assert!(text.contains("Timed out: true"),"{text}");
}

#[test]
fn review_child_inherited_pipe_outlives_timeout() {
    let w=make_temp_dir("review-descendant");
    let started=std::time::Instant::now();
    let (text, failed)=output(run_tool_call(w.path(),"t","run_command",&json!({
        "command":["sh","-c","sleep 0.4 &"],"timeout_ms":50,"approval":"workspace-write"
    })));
    assert!(!failed);
    assert!(started.elapsed().as_millis()>=350);
    assert!(text.contains("Timed out: false"));
}

#[test]
fn review_signal_death_reported_as_success() {
    let w=make_temp_dir("review-signal");
    let (text, failed)=output(run_tool_call(w.path(),"t","run_command",&json!({
        "command":["sh","-c","kill -TERM $$"],"approval":"workspace-write"
    })));
    assert!(!failed);
    assert!(text.contains("Exit code: terminated"));
}

#[test]
fn review_new_path_dotdot_escapes_workspace() {
    let parent=make_temp_dir("review-create-parent");
    let w=parent.path().join("workspace");
    fs::create_dir(&w).unwrap();
    let (text,failed)=output(run_tool_call(&w,"t","apply_patch",&json!({
        "patch":"*** Begin Patch\n*** Add File: missing/../../escaped.txt\n+nonsecret-fixture\n*** End Patch"
    })));
    assert!(!failed,"{text}");
    assert!(parent.path().join("escaped.txt").exists());
}

#[test]
fn review_partial_patch_keeps_first_write_after_second_fails() {
    let w=make_temp_dir("review-patch");
    let (_,failed)=output(run_tool_call(w.path(),"t","apply_patch",&json!({
        "patch":"*** Begin Patch\n*** Add File: first.txt\n+created\n*** Delete File: nonexistent.txt\n*** End Patch"
    })));
    assert!(failed);
    assert!(w.path().join("first.txt").exists());
}

#[test]
fn review_small_history_is_summarized_and_starts_with_system() {
    let messages:Vec<_>=(0..25).map(|i| Message::user_text(format!("message {i}"))).collect();
    let prepared=prepare_context_messages(&messages,&ConversationSummaryState::default(),ProviderKind::Anthropic,"claude-sonnet-4-5",1000,&default_tools()).unwrap();
    assert!(prepared.metrics.estimated_input_tokens < 10000);
    assert_eq!(prepared.metrics.summarized_messages,1);
    assert!(matches!(prepared.messages[0].role,Role::System));
}

#[test]
fn review_emergency_context_cut_orphans_tool_result() {
    let mut messages=vec![Message::assistant_blocks(vec![ContentBlock::ToolUse{
        id:"t".into(),name:"read_file".into(),input:json!({"path":"a"})
    }]), Message::user_blocks(vec![ContentBlock::ToolResult{
        tool_use_id:"t".into(),content:"result".into(),is_error:None
    }])];
    for i in 0..23 {messages.push(Message::user_text(format!("message {i}")));}
    let prepared=prepare_context_messages(&messages,&ConversationSummaryState::default(),ProviderKind::Anthropic,"claude-sonnet-4-5",1000,&default_tools()).unwrap();
    let value=serde_json::to_value(&prepared.messages).unwrap();
    assert_eq!(value[1]["content"][0]["type"],"tool_result");
    assert!(!value.to_string().contains("\"type\":\"tool_use\""));
}

#[tokio::test]
async fn review_second_request_failure_loses_record_of_completed_edit() {
    let w=make_temp_dir("review-failed-turn");
    let response=ProviderResponse{
        message:AgentMessageResponse{content:vec![ContentBlock::ToolUse{
            id:"t".into(),name:"apply_patch".into(),input:json!({"patch":"*** Begin Patch\n*** Add File: created.txt\n+created\n*** End Patch"})
        }]},streamed_text:false,usage:None
    };
    let provider=ScriptedProvider::new(vec![Ok(response),Err(ProviderError::Api("simulated second request failure".into()))]);
    let mut agent=crate::AgentService::with_provider(w.path().to_path_buf(),Box::new(provider),"claude-sonnet-4-5".into(),1000,reqwest::Client::new());
    assert!(agent.submit("create file".into()).await.is_err());
    assert!(w.path().join("created.txt").exists());
    assert_eq!(agent.messages.len(),1);
    assert!(!w.path().join(".socium/conversation.json").exists());
}
