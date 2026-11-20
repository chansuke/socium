//! Requests against a real provider, ignored by default because they cost
//! money. Choose a registered alias with `SOCIUM_LIVE_MODEL` (default
//! `haiku`) and run:
//!
//! `SOCIUM_LIVE_MODEL=ds-flash cargo test -p socium-core live_ -- --ignored`
//!
//! The registry and keys come from the real environment, as at startup.
#![expect(
    clippy::disallowed_methods,
    reason = "live tests read the real provider key and model from the environment"
)]

use crate::constants::TEXT_EDITOR_TOOL_NAME;
use crate::conversation::{AgentMessageRequest, ContentBlock, Message};
use crate::models::{ModelConfig, ModelRegistry};
use crate::providers::{Provider, ProviderResponse, build_provider};
use crate::tools::tools_for_model;
use std::env;

const LIVE_MAX_TOKENS: u32 = 4_096;

fn live_target() -> (ModelConfig, Box<dyn Provider>) {
    let lookup = |key: &str| env::var(key).ok();
    let registry = ModelRegistry::load(&lookup).expect("models.toml");
    let alias = env::var("SOCIUM_LIVE_MODEL").unwrap_or_else(|_| "haiku".to_string());
    let model = registry
        .find(&alias)
        .unwrap_or_else(|| panic!("'{alias}' is not registered"))
        .clone();
    let provider = build_provider(registry.provider(&model.provider).unwrap(), &model, &lookup)
        .unwrap_or_else(|err| panic!("cannot build '{alias}': {err}"));
    (model, provider)
}

async fn ask(prompt: &str) -> (ModelConfig, ProviderResponse) {
    let (model, provider) = live_target();
    let messages = [Message::user_text(prompt.to_string())];
    let tools = tools_for_model();
    let max_tokens = model
        .max_output_tokens
        .unwrap_or(LIVE_MAX_TOKENS)
        .min(LIVE_MAX_TOKENS);
    let request = AgentMessageRequest::new(&model.id, max_tokens, &messages, &tools);
    let mut ignore = |_| {};
    let response = provider
        .send_message(
            &crate::provider_http_client().unwrap(),
            &request,
            &mut ignore,
        )
        .await
        .unwrap_or_else(|err| panic!("{} ({}): {err}", model.alias, model.id));
    (model, response)
}

fn tool_call(response: &ProviderResponse) -> Option<(&str, &serde_json::Value)> {
    response
        .message
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolUse { name, input, .. } => Some((name.as_str(), input)),
            _ => None,
        })
}

#[tokio::test]
#[ignore = "calls a paid API; set SOCIUM_LIVE_MODEL and run with --ignored"]
async fn live_text_reply_streams_with_usage() {
    let (model, response) = ask("Reply with the single word: pong").await;
    let text = response
        .message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert!(
        text.to_lowercase().contains("pong"),
        "{}: {text:?}",
        model.alias
    );
    let usage = response.usage.expect("usage");
    assert!(
        usage.input_tokens > 0 && usage.output_tokens > 0,
        "{usage:?}"
    );
}

#[tokio::test]
#[ignore = "calls a paid API; set SOCIUM_LIVE_MODEL and run with --ignored"]
async fn live_schema_tool_call_round_trips_its_input() {
    let (model, response) =
        ask("Call the read_file tool with the path README.md. Do not answer in text.").await;
    let (name, input) =
        tool_call(&response).unwrap_or_else(|| panic!("{}: no tool call", model.alias));
    assert_eq!(name, "read_file");
    assert_eq!(input["path"], "README.md");
}

#[tokio::test]
#[ignore = "calls a paid API; set SOCIUM_LIVE_MODEL and run with --ignored"]
async fn live_tool_result_round_trip_completes_the_turn() {
    // A prompt that needs reasoning before the call, so reasoning models
    // produce reasoning that the second request must carry back.
    let (model, provider) = live_target();
    let tools = tools_for_model();
    let mut messages = vec![Message::user_text(
        "Let n be the number of primes between 1000 and 1100. Work it out carefully, then \
         call read_file with the path \"notes/n<n>.md\" (for example notes/n3.md). Call the \
         tool before answering, then reply with the file's first heading."
            .to_string(),
    )];
    let client = crate::provider_http_client().unwrap();
    let mut ignore = |_| {};
    let first = {
        let request = AgentMessageRequest::new(&model.id, LIVE_MAX_TOKENS, &messages, &tools);
        provider
            .send_message(&client, &request, &mut ignore)
            .await
            .unwrap_or_else(|err| panic!("{} first request: {err}", model.alias))
    };
    let call_id = first
        .message
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolUse { id, name, .. } if name == "read_file" => Some(id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{}: no read_file call", model.alias));
    let thinking = first
        .message
        .content
        .iter()
        .filter(|block| matches!(block, ContentBlock::Thinking { .. }))
        .count();
    messages.push(Message::assistant_blocks(first.message.content));
    messages.push(Message::user_blocks(vec![ContentBlock::ToolResult {
        tool_use_id: call_id,
        content: "# Prime Notes\n\nNothing else here.".into(),
        is_error: None,
    }]));
    let request = AgentMessageRequest::new(&model.id, LIVE_MAX_TOKENS, &messages, &tools);
    let second = provider
        .send_message(&client, &request, &mut ignore)
        .await
        .unwrap_or_else(|err| panic!("{} second request: {err}", model.alias));
    let text = second
        .message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    eprintln!(
        "{}: {thinking} thinking block(s) sent back; usage {:?} then {:?}",
        model.alias, first.usage, second.usage
    );
    assert!(text.contains("Prime Notes"), "{}: {text:?}", model.alias);
}

/// Replaces the first request with a summary, as compaction does, and
/// sends the kept turn back. Kept thinking would fail the prefix check on
/// models with preserved thinking (Claude Opus 5.5 and others), which can
/// answer 400; the prepared context must leave it out.
#[tokio::test]
#[ignore = "calls a paid API; set SOCIUM_LIVE_MODEL and run with --ignored"]
async fn live_request_after_a_summary_is_accepted() {
    use crate::agent::state::{ConversationSummary, ConversationSummaryState};
    let (model, provider) = live_target();
    let tools = tools_for_model();
    let client = crate::provider_http_client().unwrap();
    let mut ignore = |_| {};
    let mut messages = vec![Message::user_text(
        "Think it through, then call read_file with the path notes/a.md before answering."
            .to_string(),
    )];
    let first = {
        let request = AgentMessageRequest::new(&model.id, LIVE_MAX_TOKENS, &messages, &tools);
        provider
            .send_message(&client, &request, &mut ignore)
            .await
            .unwrap_or_else(|err| panic!("{} first request: {err}", model.alias))
    };
    let call_id = first
        .message
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{}: no tool call", model.alias));
    messages.push(Message::assistant_blocks(first.message.content));
    messages.push(Message::user_blocks(vec![ContentBlock::ToolResult {
        tool_use_id: call_id,
        content: "# A\n".into(),
        is_error: None,
    }]));
    messages.push(Message::user_text("Reply with the heading only.".into()));
    let summary = ConversationSummaryState {
        summary: Some(ConversationSummary {
            covered_messages: 1,
            text: "1. User request: read notes/a.md".into(),
            thinking_valid_from: Some(messages.len()),
            ..Default::default()
        }),
    };
    let prepared = crate::agent::context::prepare_context_messages(
        &messages,
        &summary,
        crate::agent::context::resolve_context_budget(model.context_window, LIVE_MAX_TOKENS),
        &model.id,
        &tools,
    )
    .unwrap();
    let request = AgentMessageRequest::new(&model.id, LIVE_MAX_TOKENS, &prepared.messages, &tools);
    provider
        .send_message(&client, &request, &mut ignore)
        .await
        .unwrap_or_else(|err| panic!("{} request after summary: {err}", model.alias));
}

#[tokio::test]
#[ignore = "calls a paid API; set SOCIUM_LIVE_MODEL and run with --ignored"]
async fn live_text_editor_call_uses_the_editor_input() {
    let (model, response) = ask(&format!(
        "Use the {TEXT_EDITOR_TOOL_NAME} tool with the view command to show README.md. Do not answer in text."
    ))
    .await;
    let (name, input) =
        tool_call(&response).unwrap_or_else(|| panic!("{}: no tool call", model.alias));
    assert_eq!(name, TEXT_EDITOR_TOOL_NAME);
    assert_eq!(input["command"], "view");
    assert_eq!(input["path"], "README.md");
}

/// The whole delegation path on a real model: the main agent is asked to
/// use an explorer, the subagent reads the workspace on its own, and its
/// report reaches the reply. Also shows whether the tool description is
/// enough for the model to delegate when asked.
#[tokio::test]
#[ignore = "calls a paid API; set SOCIUM_LIVE_MODEL and run with --ignored"]
async fn live_main_agent_delegates_to_an_explorer() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("src")).unwrap();
    std::fs::write(
        workspace.path().join("src/config.rs"),
        "pub const RETRY_LIMIT: u32 = 7;\n",
    )
    .unwrap();
    let alias = env::var("SOCIUM_LIVE_MODEL").unwrap_or_else(|_| "haiku".to_string());
    let lookup: crate::EnvLookup = std::sync::Arc::new(move |key: &str| {
        if key == "MODEL" {
            Some(alias.clone())
        } else {
            env::var(key).ok()
        }
    });
    let registry = ModelRegistry::load(lookup.as_ref()).expect("models.toml");
    let mut service = crate::AgentService::open(
        workspace.path().to_path_buf(),
        crate::session::SessionStart::New,
        registry,
        lookup,
        None,
        crate::provider_http_client().unwrap(),
    )
    .unwrap();
    let mut subagent_tools = Vec::new();
    let mut delegated = false;

    let reply = service
        .submit_with_progress(
            "Use the agent tool with agent_type explorer to find the value of RETRY_LIMIT in this workspace. Do not search yourself. Then tell me the value.".into(),
            |event| {
                if let crate::AgentProgress::Activity(activity) = event
                    && let crate::ActivityKind::ToolStarted { name, .. } = activity.kind
                {
                    match activity.agent {
                        Some(_) => subagent_tools.push(name),
                        None => delegated |= name == "agent",
                    }
                }
            },
        )
        .await
        .unwrap();

    assert!(
        delegated,
        "the main agent did not call agent: {}",
        reply.text
    );
    assert!(!subagent_tools.is_empty(), "the subagent used no tools");
    assert!(
        subagent_tools
            .iter()
            .all(|name| crate::agent::subagent::catalog::ToolSet::ReadOnly.allows(name)),
        "{subagent_tools:?}"
    );
    assert!(reply.text.contains('7'), "{}", reply.text);
    let runs = service.subagent_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, crate::SubagentStatus::Finished);
    println!("subagent tools: {subagent_tools:?}\nreply: {}", reply.text);
}

/// One real search, billed to `DEEPSEEK_API_KEY`.
#[tokio::test]
#[ignore = "sends a paid request to DeepSeek's Search API"]
async fn live_web_search() {
    let lookup = |key: &str| env::var(key).ok();
    let settings = crate::settings::WebSearchSettings {
        base_url: crate::settings::DEEPSEEK_SEARCH_BASE_URL.into(),
    };
    let search = crate::tools::web_search::WebSearch::new(&settings, &lookup).unwrap();
    let text = search
        .run(
            &crate::provider_http_client().unwrap(),
            &crate::TaskControl::default(),
            &serde_json::json!({"query": "Rust programming language site:rust-lang.org", "count": 3}),
        )
        .await
        .unwrap();
    assert!(text.contains("https://"), "{text}");
}

/// Three compactions in a row, each extending the last summary, over a
/// session where the user corrects a requirement, a long test run fails
/// at its very end, and the final check is promised but never run. The
/// summary must keep the newer requirement, the error from the end of the
/// output, and that the tests have not passed.
#[tokio::test]
#[ignore = "calls a paid API; set SOCIUM_LIVE_MODEL and run with --ignored"]
async fn live_repeated_compaction_keeps_the_latest_correction_and_unverified_checks() {
    use crate::agent::compaction::{CompactionRequest, summarize_with_llm};

    let (model, provider) = live_target();
    let client = crate::provider_http_client().unwrap();
    let call = |id: &str, name: &str, input: serde_json::Value| ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
    };
    let result = |id: &str, content: String, is_error: bool| {
        Message::user_blocks(vec![ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content,
            is_error: is_error.then_some(true),
        }])
    };
    let test_output = format!(
        "{}\nerror[E0308]: mismatched types\n --> src/cli.rs:42:19\nerror: could not compile `app`",
        (1..=150)
            .map(|n| format!("   Compiling dep_{n} v0.1.0"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let segments = [
        vec![
            Message::user_text("Add a --port flag to src/cli.rs. Its default is 8080.".into()),
            Message::assistant_blocks(vec![
                ContentBlock::Text {
                    text: "Adding the flag with default 8080.".into(),
                },
                call(
                    "e1",
                    "apply_patch",
                    serde_json::json!({"patch": "*** Update File: src/cli.rs"}),
                ),
            ]),
            result("e1", "Updated src/cli.rs".into(), false),
        ],
        vec![
            Message::user_text(
                "Correction: the default port must be 9090, not 8080. Then run cargo test.".into(),
            ),
            Message::assistant_blocks(vec![call(
                "e2",
                "apply_patch",
                serde_json::json!({"patch": "*** Update File: src/cli.rs"}),
            )]),
            result("e2", "Updated src/cli.rs".into(), false),
            Message::assistant_blocks(vec![call(
                "t1",
                "run_command",
                serde_json::json!({"command": ["cargo", "test"]}),
            )]),
            result("t1", format!("exit 101\nstdout:\n{test_output}"), true),
        ],
        vec![
            Message::user_text("Also document the flag in README.md.".into()),
            Message::assistant_blocks(vec![
                ContentBlock::Text {
                    text: "I will document it, then fix the build and run cargo test again.".into(),
                },
                call(
                    "e3",
                    "apply_patch",
                    serde_json::json!({"patch": "*** Update File: README.md"}),
                ),
            ]),
            result("e3", "Updated README.md".into(), false),
        ],
    ];

    let mut summary: Option<String> = None;
    for segment in &segments {
        let request = CompactionRequest {
            previous_summary: summary.as_deref(),
            messages: segment,
        };
        let (text, _) = summarize_with_llm(provider.as_ref(), &model.id, &client, &request).await;
        summary = Some(text.unwrap_or_else(|err| panic!("{}: {err}", model.alias)));
    }
    let summary = summary.unwrap();
    eprintln!("{}:\n{summary}", model.alias);
    let lower = summary.to_lowercase();
    assert!(
        summary.contains("9090"),
        "{}: lost the correction",
        model.alias
    );
    assert!(
        summary.contains("E0308"),
        "{}: lost the error at the end of the output",
        model.alias
    );
    for claim in [
        "tests pass",
        "tests passed",
        "all tests pass",
        "build succeeded",
    ] {
        assert!(!lower.contains(claim), "{}: claims '{claim}'", model.alias);
    }
}
