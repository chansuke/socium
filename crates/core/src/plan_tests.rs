//! The plan through `AgentService`: a turn that records one, the events
//! the Timeline reads, rewind, compaction, and subagents.

use crate::constants::PLAN_TOOL_NAME;
use crate::conversation::{AgentMessageResponse, ContentBlock, Message, MessageContent};
use crate::providers::ProviderResponse;
use crate::providers::error::ProviderError;
use crate::state::{ConversationSummary, ConversationSummaryState};
use crate::test_support::{ScriptedProvider, make_temp_dir, text_provider_response};
use crate::{ActivityKind, AgentProgress, AgentService, RewindMode};
use serde_json::{Value, json};

fn service(workspace: &std::path::Path, provider: ScriptedProvider) -> AgentService {
    AgentService::with_provider(
        workspace.to_path_buf(),
        Box::new(provider),
        "claude-sonnet-4-5".to_string(),
        1_000,
        crate::test_support::http_client(),
    )
}

fn plan_call(id: &str, input: Value) -> Result<ProviderResponse, ProviderError> {
    Ok(ProviderResponse {
        message: AgentMessageResponse {
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: PLAN_TOOL_NAME.into(),
                input,
            }],
            ..Default::default()
        },
        streamed_text: false,
        usage: None,
    })
}

fn start() -> Value {
    json!({"goal": "Fix the parser", "tasks": [
        {"id": "repro", "title": "Reproduce", "status": "active"},
        {"id": "fix", "title": "Fix it"}
    ]})
}

/// The text of the tool result `id` in a recorded request.
fn sent_result(request: &Value, id: &str) -> String {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .find(|block| block["tool_use_id"] == id)
        .map(|block| block["content"].as_str().unwrap().to_string())
        .unwrap()
}

#[tokio::test]
async fn a_turn_records_the_plan_and_the_timeline_hears_of_it_after_the_call() {
    let workspace = make_temp_dir("plan_turn");
    let provider = ScriptedProvider::new(vec![
        plan_call("p1", start()),
        Ok(text_provider_response("Planned.", None)),
    ]);
    let requests = provider.recorded_requests.clone();
    let mut service = service(workspace.path(), provider);
    assert!(service.plan().is_none());

    let mut events = Vec::new();
    service
        .submit_with_progress("fix the parser".into(), |event| {
            if let AgentProgress::Activity(event) = event {
                events.push(event.kind);
            }
        })
        .await
        .unwrap();

    let plan = service.plan().unwrap();
    assert_eq!(plan.goal, "Fix the parser");
    assert_eq!(plan.counts.active, 1);
    let receipt = sent_result(&requests.lock().unwrap()[1], "p1");
    assert!(
        receipt.starts_with("Plan r1 \"Fix the parser\": 0% (0 of 2 tasks done), 1 active"),
        "{receipt}"
    );

    let finished = events
        .iter()
        .position(|kind| matches!(kind, ActivityKind::ToolFinished { name, .. } if name == PLAN_TOOL_NAME))
        .unwrap();
    let updated = events
        .iter()
        .position(|kind| matches!(kind, ActivityKind::PlanUpdated { plan: sent } if *sent == plan))
        .unwrap();
    assert!(finished < updated);
    // Writing the plan down is not a phase of the work.
    assert!(events.iter().any(|kind| matches!(
        kind,
        ActivityKind::ToolStarted { input_summary, .. } if input_summary == "goal: Fix the parser"
    )));
}

#[tokio::test]
async fn a_refused_update_leaves_the_plan_and_tells_the_model_why() {
    let workspace = make_temp_dir("plan_refused");
    let provider = ScriptedProvider::new(vec![
        plan_call("p1", start()),
        plan_call("p2", json!({"tasks": [{"id": "repro", "status": "done"}]})),
        Ok(text_provider_response("ok", None)),
    ]);
    let requests = provider.recorded_requests.clone();
    let mut service = service(workspace.path(), provider);

    service.submit("fix it".into()).await.unwrap();

    let error = sent_result(&requests.lock().unwrap()[2], "p2");
    assert!(error.contains("say why in note"), "{error}");
    assert_eq!(service.plan().unwrap().revision, 1);
}

#[tokio::test]
async fn rewinding_the_conversation_takes_the_plan_back_with_it() {
    let workspace = make_temp_dir("plan_rewind");
    let provider = ScriptedProvider::new(vec![
        plan_call("p1", start()),
        Ok(text_provider_response("Planned.", None)),
        plan_call(
            "p2",
            json!({"tasks": [{"id": "repro", "status": "done", "note": "fails on empty input"}]}),
        ),
        Ok(text_provider_response("Reproduced.", None)),
    ]);
    let mut service = service(workspace.path(), provider);
    service.submit("plan it".into()).await.unwrap();
    service.submit("go on".into()).await.unwrap();
    assert_eq!(service.plan().unwrap().counts.done, 1);

    let second = service.rewind_points().last().unwrap().turn_id;
    service.rewind(second, RewindMode::Conversation).unwrap();
    assert_eq!(service.plan().unwrap().counts.done, 0);

    let first = service.rewind_points().first().unwrap().turn_id;
    service.rewind(first, RewindMode::Conversation).unwrap();
    assert!(service.plan().is_none());
}

#[tokio::test]
async fn the_plan_survives_a_restart() {
    let workspace = make_temp_dir("plan_resume");
    let provider = ScriptedProvider::new(vec![
        plan_call("p1", start()),
        Ok(text_provider_response("Planned.", None)),
    ]);
    let mut service = service(workspace.path(), provider);
    service.submit("plan it".into()).await.unwrap();
    let saved: crate::state::ConversationState =
        crate::state::load_json_file(&service.conversation_path()).unwrap();

    let mut reopened = self::service(workspace.path(), ScriptedProvider::new(Vec::new()));
    reopened.messages = saved.messages;
    assert_eq!(reopened.plan(), service.plan());
}

#[test]
fn after_compaction_the_summary_carries_the_plan_at_the_cut() {
    let mut raw = vec![
        Message::user_text("plan it".into()),
        Message::assistant_blocks(vec![ContentBlock::ToolUse {
            id: "p1".into(),
            name: PLAN_TOOL_NAME.into(),
            input: start(),
        }]),
        Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: "p1".into(),
            content: "receipt".into(),
            is_error: None,
        }]),
        Message::assistant_blocks(vec![ContentBlock::Text {
            text: "Planned.".into(),
        }]),
    ];
    raw.extend((0..6).map(|n| Message::user_text(format!("later {n}"))));
    let summary = ConversationSummaryState {
        summary: Some(ConversationSummary {
            covered_messages: 4,
            text: "1. User request: fix the parser".into(),
            ..Default::default()
        }),
    };
    let prepared = crate::agent::context::prepare_context_messages(
        &raw,
        &summary,
        crate::agent::context::resolve_context_budget(None, 1_000),
        "claude-sonnet-4-5",
        &[],
    )
    .unwrap();

    let MessageContent::Text(first) = &prepared.messages[0].content else {
        panic!("the summary comes first");
    };
    assert!(first.contains("The plan at this point"), "{first}");
    assert!(first.contains("* repro [active] Reproduce"), "{first}");
    // The calls it covers are no longer sent.
    assert!(prepared.messages.iter().all(|message| !matches!(
        &message.content,
        MessageContent::Blocks(blocks) if blocks.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }))
    )));
}

#[test]
fn only_the_main_agent_is_offered_update_plan() {
    let tools = crate::tools::tools_for_model();
    assert!(tools.iter().any(|tool| tool.name() == PLAN_TOOL_NAME));
    let catalog = crate::agent::subagent::catalog::SubagentCatalog::builtin();
    assert!(!catalog.names().is_empty());
    for kind in catalog.names() {
        let set = &catalog.find(kind).unwrap().tools;
        assert!(!set.allows(PLAN_TOOL_NAME), "{kind}");
    }
}

#[tokio::test]
async fn rounds_without_an_update_bring_one_reminder_in_the_last_result() {
    let workspace = make_temp_dir("plan_nudge");
    let list = |id: &str| {
        Ok(ProviderResponse {
            message: AgentMessageResponse {
                content: vec![ContentBlock::ToolUse {
                    id: id.into(),
                    name: "list_files".into(),
                    input: json!({}),
                }],
                ..Default::default()
            },
            streamed_text: false,
            usage: None,
        })
    };
    let rounds = crate::constants::PLAN_NUDGE_ROUNDS;
    let mut script = vec![plan_call("p1", start())];
    script.extend((0..rounds).map(|n| list(&format!("l{n}"))));
    script.push(Ok(text_provider_response("done", None)));
    let provider = ScriptedProvider::new(script);
    let requests = provider.recorded_requests.clone();
    let mut service = service(workspace.path(), provider);

    let mut nudges = 0;
    service
        .submit_with_progress("explore".into(), |event| {
            if let AgentProgress::Activity(event) = event
                && matches!(event.kind, ActivityKind::PlanNudged { .. })
            {
                nudges += 1;
            }
        })
        .await
        .unwrap();

    // The plan call's own round counts, so the reminder follows the
    // (rounds - 1)th listing.
    let requests = requests.lock().unwrap();
    let reminded = format!("l{}", rounds - 2);
    assert!(sent_result(&requests[rounds], &reminded).contains("(Plan check:"));
    assert!(!sent_result(&requests[rounds], &format!("l{}", rounds - 3)).contains("(Plan check:"));
    assert_eq!(nudges, 1);
    // Saved with the result, so a restart sends the same history.
    assert!(service.messages.iter().any(|message| matches!(
        &message.content,
        MessageContent::Blocks(blocks) if blocks.iter().any(|b| matches!(b, ContentBlock::ToolResult { content, .. } if content.contains("(Plan check:")))
    )));
}
