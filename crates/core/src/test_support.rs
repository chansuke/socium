use crate::conversation::{AgentMessageRequest, AgentMessageResponse, ContentBlock};
use crate::providers::error::ProviderError;
use crate::providers::{
    Provider, ProviderCapabilities, ProviderEvent, ProviderKind, ProviderResponse, TokenUsage,
};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Scripted `Provider` for tests: returns pre-recorded responses in order
/// and records every serialized request it receives.
pub(crate) struct ScriptedProvider {
    responses: Mutex<VecDeque<Result<ProviderResponse, ProviderError>>>,
    pub(crate) recorded_requests: Arc<Mutex<Vec<serde_json::Value>>>,
    pub(crate) supports_tools: bool,
}

impl ScriptedProvider {
    pub(crate) fn new(responses: Vec<Result<ProviderResponse, ProviderError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            recorded_requests: Arc::default(),
            supports_tools: true,
        }
    }
}

pub(crate) fn text_provider_response(text: &str, usage: Option<TokenUsage>) -> ProviderResponse {
    ProviderResponse {
        message: AgentMessageResponse {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            ..Default::default()
        },
        streamed_text: false,
        usage,
    }
}

#[async_trait::async_trait]
impl Provider for ScriptedProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_tools: self.supports_tools,
        }
    }

    /// Tests script their own histories; every block goes back.
    fn reads_thinking_from(&self, _: Option<&crate::conversation::Origin>) -> bool {
        true
    }

    async fn send_message(
        &self,
        _client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
        _on_delta: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<ProviderResponse, ProviderError> {
        let value = serde_json::to_value(request)?;
        if let Err(problem) = check_request(&value) {
            // A test must not pass by sending what no provider accepts.
            panic!("ScriptedProvider got a request no provider would accept: {problem}\n{value:#}");
        }
        self.recorded_requests.lock().unwrap().push(value);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Err(ProviderError::Api(
                    "ScriptedProvider ran out of scripted responses".to_string(),
                ))
            })
    }
}

/// What makes a request one no provider accepts: a role other than user or
/// assistant, an empty message or text block, a history that does not start
/// and end with the user, a tool call without its result in the next message
/// or a result without its call, a tool input that is not an object, and
/// tool definitions with duplicate names or a non-object schema.
pub(crate) fn check_request(request: &serde_json::Value) -> Result<(), String> {
    use std::collections::HashSet;
    if request["max_tokens"].as_u64().is_none_or(|max| max == 0) {
        return Err("max_tokens must be positive".into());
    }
    let mut names = HashSet::new();
    for tool in request["tools"].as_array().into_iter().flatten() {
        let name = tool["name"].as_str().ok_or("a tool has no name")?;
        if !names.insert(name) {
            return Err(format!("tool {name} is defined twice"));
        }
        if tool.get("type").is_none() && tool["input_schema"]["type"] != "object" {
            return Err(format!("tool {name} has no object input_schema"));
        }
    }
    let messages = request["messages"]
        .as_array()
        .ok_or("messages is not a list")?;
    let role = |index: usize| messages[index]["role"].as_str().unwrap_or_default();
    if messages.is_empty() || role(0) != "user" || role(messages.len() - 1) != "user" {
        return Err("the history must start and end with a user message".into());
    }
    // Tool calls of the previous assistant message, which this one answers.
    let mut open: HashSet<String> = HashSet::new();
    for (index, message) in messages.iter().enumerate() {
        let role = role(index);
        if role != "user" && role != "assistant" {
            return Err(format!("message {index} has role '{role}'"));
        }
        let blocks = match &message["content"] {
            serde_json::Value::String(text) if text.is_empty() => {
                return Err(format!("message {index} is empty"));
            }
            serde_json::Value::String(_) => Vec::new(),
            serde_json::Value::Array(blocks) if blocks.is_empty() => {
                return Err(format!("message {index} has no content blocks"));
            }
            serde_json::Value::Array(blocks) => blocks.clone(),
            other => return Err(format!("message {index} has content {other}")),
        };
        let mut calls = HashSet::new();
        let mut answered = HashSet::new();
        for block in &blocks {
            match block["type"].as_str().unwrap_or_default() {
                "text" if block["text"].as_str().is_none_or(str::is_empty) => {
                    return Err(format!("message {index} has an empty text block"));
                }
                "tool_use" => {
                    let id = block["id"].as_str().unwrap_or_default();
                    if role != "assistant" || id.is_empty() || !calls.insert(id.to_string()) {
                        return Err(format!(
                            "message {index} has a misplaced or repeated tool_use '{id}'"
                        ));
                    }
                    if !block["input"].is_object() {
                        return Err(format!("tool_use {id} has input {}", block["input"]));
                    }
                }
                "tool_result" => {
                    let id = block["tool_use_id"].as_str().unwrap_or_default();
                    if role != "user" || !open.contains(id) || !answered.insert(id.to_string()) {
                        return Err(format!(
                            "message {index} has a tool_result '{id}' without its call"
                        ));
                    }
                }
                _ => {}
            }
        }
        if answered != open {
            return Err(format!(
                "message {index} leaves tool calls of the message before it without results"
            ));
        }
        open = calls;
    }
    Ok(())
}

/// Creates a FIFO, which blocks a plain `open` for reading until a writer
/// appears.
#[cfg(unix)]
pub(crate) fn make_fifo(path: &Path) {
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `name` is a valid NUL-terminated path for the call.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
}

pub(crate) fn make_temp_dir(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("socium_{prefix}_"))
        .tempdir()
        .expect("failed to create temp dir")
}

pub(crate) fn init_git_repo(path: &Path) {
    run_git(path, &["init", "-b", "main"]);
    run_git(path, &["config", "user.email", "socium@example.com"]);
    run_git(path, &["config", "user.name", "Socium Tests"]);
    fs::write(path.join("tracked.txt"), "initial\n").unwrap();
    run_git(path, &["add", "tracked.txt"]);
    run_git(path, &["commit", "-m", "initial commit"]);
}

pub(crate) fn run_git(path: &Path, args: &[&str]) {
    let mut command = std::process::Command::new("git");
    isolate_git_config(&mut command);
    let output = command
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Fixtures exercise their own local configuration, not a developer's signing,
/// filter, or CI runner setup. Do not mutate the process-wide environment.
pub(crate) fn isolate_git_config(command: &mut std::process::Command) {
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS");
}

pub(crate) fn git_stdout(path: &Path, args: &[&str]) -> String {
    let mut command = std::process::Command::new("git");
    isolate_git_config(&mut command);
    let output = command
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The client production builds, so tests send through the same redirect
/// policy and timeouts.
pub(crate) fn http_client() -> reqwest::Client {
    crate::provider_http_client().expect("the provider HTTP client builds")
}

#[cfg(test)]
mod request_checks {
    use super::check_request;
    use serde_json::{Value, json};

    fn request(messages: Value) -> Value {
        json!({"model": "m", "max_tokens": 100, "system": "", "messages": messages,
               "tools": [{"name": "read_file", "description": "", "input_schema": {"type": "object"}}]})
    }

    fn call(id: &str, input: Value) -> Value {
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": "read_file", "input": input}]})
    }

    fn result(id: &str) -> Value {
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]})
    }

    #[test]
    fn a_well_formed_tool_round_trip_passes() {
        let messages = json!([{"role": "user", "content": "read a.rs"}, call("t1", json!({"path": "a.rs"})), result("t1")]);
        assert_eq!(check_request(&request(messages)), Ok(()));
    }

    #[test]
    fn what_no_provider_accepts_is_refused() {
        let user = json!({"role": "user", "content": "hi"});
        let cases = [
            (json!([]), "start and end"),
            (
                json!([user, {"role": "assistant", "content": "hello"}]),
                "start and end",
            ),
            (
                json!([{"role": "system", "content": "x"}, user]),
                "start and end",
            ),
            (
                json!([user, {"role": "tool", "content": "x"}, user]),
                "role 'tool'",
            ),
            (json!([{"role": "user", "content": ""}]), "is empty"),
            (
                json!([{"role": "user", "content": []}]),
                "no content blocks",
            ),
            (
                json!([{"role": "user", "content": [{"type": "text", "text": ""}]}]),
                "empty text block",
            ),
            (
                json!([user, call("t1", json!({})), user]),
                "without results",
            ),
            (json!([user, result("t9")]), "without its call"),
            (
                json!([user, call("t1", json!({})), result("t1"), {"role": "assistant", "content": "ok"}, result("t1")]),
                "without its call",
            ),
            (
                json!([user, call("t1", json!("a.rs")), result("t1")]),
                "has input",
            ),
            (
                json!([user, {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "read_file", "input": {}},
                {"type": "tool_use", "id": "t1", "name": "read_file", "input": {}}]}, result("t1")]),
                "repeated tool_use",
            ),
        ];
        for (messages, expected) in cases {
            let err = check_request(&request(messages.clone())).unwrap_err();
            assert!(err.contains(expected), "{messages}: {err}");
        }
        let mut twice = request(json!([user]));
        twice["tools"] = json!([{"name": "a", "input_schema": {"type": "object"}}, {"name": "a", "input_schema": {"type": "object"}}]);
        assert!(check_request(&twice).unwrap_err().contains("defined twice"));
        let mut zero = request(json!([user]));
        zero["max_tokens"] = json!(0);
        assert!(check_request(&zero).is_err());
    }
}
