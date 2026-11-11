use crate::agent::types::{AgentMessageRequest, AgentMessageResponse, ContentBlock};
use crate::providers::error::ProviderError;
use crate::providers::{
    DeltaEvent, Provider, ProviderCapabilities, ProviderKind, ProviderResponse, TokenUsage,
};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

/// Scripted `Provider` for tests: returns pre-recorded responses in order
/// and records every serialized request it receives.
pub(crate) struct ScriptedProvider {
    responses: Mutex<VecDeque<Result<ProviderResponse, ProviderError>>>,
    pub(crate) recorded_requests: Mutex<Vec<serde_json::Value>>,
}

impl ScriptedProvider {
    pub(crate) fn new(responses: Vec<Result<ProviderResponse, ProviderError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            recorded_requests: Mutex::new(Vec::new()),
        }
    }
}

pub(crate) fn text_provider_response(text: &str, usage: Option<TokenUsage>) -> ProviderResponse {
    ProviderResponse {
        message: AgentMessageResponse {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
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
            supports_tools: true,
        }
    }

    async fn send_message(
        &self,
        _client: &reqwest::Client,
        request: &AgentMessageRequest<'_>,
        _on_delta: &mut (dyn FnMut(DeltaEvent) + Send),
    ) -> Result<ProviderResponse, ProviderError> {
        self.recorded_requests
            .lock()
            .unwrap()
            .push(serde_json::to_value(request)?);
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
