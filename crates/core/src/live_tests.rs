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
