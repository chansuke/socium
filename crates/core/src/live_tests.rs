//! Requests against a real provider, ignored by default because they cost
//! money. Choose a registered alias with `SOCIUM_LIVE_MODEL` (default
//! `haiku`) and run:
//!
//! `SOCIUM_LIVE_MODEL=ds-flash cargo test -p socium-core live_ -- --ignored`
//!
//! The registry and keys come from the real environment, as at startup.

use crate::agent::types::{AgentMessageRequest, ContentBlock, Message};
use crate::constants::TEXT_EDITOR_TOOL_NAME;
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
    let tools = tools_for_model(model.features.anthropic_text_editor);
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
    let tools = tools_for_model(model.features.anthropic_text_editor);
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
