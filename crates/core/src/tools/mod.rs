use crate::conversation::ToolDefinition;
use serde_json::Value;

pub(crate) mod apply_patch;
pub(crate) mod diff;
pub(crate) mod edit_history;
pub(crate) mod find_symbol;
pub(crate) mod git;
pub(crate) mod input;
pub(crate) mod list_files;
pub(crate) mod read_file;
pub(crate) mod ripgrep;
pub(crate) mod run_command;
pub(crate) mod search_text;
pub(crate) mod text_editor;
pub(crate) mod text_file;
pub(crate) mod web_search;

use crate::AgentProgress;
use crate::constants::TEXT_EDITOR_TOOL_NAME;
use crate::conversation::ContentBlock;
use crate::tool_error::ToolError;
use crate::tools::apply_patch::ApplyPatchTool;
use crate::tools::find_symbol::FindSymbolTool;
use crate::tools::git::{GitBranchTool, GitCommitTool, GitDiffTool, GitLogTool, GitStatusTool};
use crate::tools::list_files::ListFilesTool;
use crate::tools::read_file::ReadFileTool;
use crate::tools::run_command::RunCommandTool;
use crate::tools::search_text::SearchTextTool;
use crate::tools::text_editor::TextEditorTool;
use std::path::Path;

pub(crate) struct ToolContext<'a> {
    pub(crate) workspace_root: &'a Path,
    pub(crate) control: &'a crate::TaskControl,
    pub(crate) settings: &'a crate::settings::Settings,
    /// What a command nobody reviewed may not touch; `None` when a person
    /// approved this call or it needs no review.
    pub(crate) restrictions: Option<&'a crate::approval::Restrictions>,
    /// The turn an edit is recorded under; `None` outside a turn.
    pub(crate) edit_origin: Option<&'a crate::tools::edit_history::EditOrigin>,
}

/// A built-in tool: its wire schema and its execution, colocated so the
/// name/schema/implementation cannot drift apart.
pub(crate) trait Tool: Sync {
    fn name(&self) -> &'static str;
    fn definition(&self) -> ToolDefinition;
    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError>;
}

static BUILTIN_TOOLS: &[&dyn Tool] = &[
    &ReadFileTool,
    &ListFilesTool,
    &SearchTextTool,
    &ApplyPatchTool,
    &GitBranchTool,
    &GitStatusTool,
    &GitDiffTool,
    &GitLogTool,
    &GitCommitTool,
    &FindSymbolTool,
    &RunCommandTool,
    &TextEditorTool,
];

/// Which built-in tools the model is offered, from `[tools] profile` in
/// config.toml, so tool sets can be compared. `agent`, `update_plan`, and
/// `web_search` are offered whatever the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ToolProfile {
    #[default]
    Full,
    Basic,
    Search,
}

const FULL_PROFILE: &[&str] = &[
    "read_file",
    "list_files",
    "search_text",
    "apply_patch",
    "git_branch",
    "git_status",
    "git_diff",
    "git_log",
    "git_commit",
    "find_symbol",
    "run_command",
    TEXT_EDITOR_TOOL_NAME,
];

const BASIC_PROFILE: &[&str] = &[
    "read_file",
    "apply_patch",
    TEXT_EDITOR_TOOL_NAME,
    "run_command",
    "git_commit",
];

const SEARCH_PROFILE: &[&str] = &[
    "read_file",
    "apply_patch",
    TEXT_EDITOR_TOOL_NAME,
    "run_command",
    "git_commit",
    "search_text",
    "find_symbol",
];

impl ToolProfile {
    pub(crate) const ALL: [Self; 3] = [Self::Full, Self::Basic, Self::Search];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Basic => "basic",
            Self::Search => "search",
        }
    }

    pub(crate) fn tools(self) -> &'static [&'static str] {
        match self {
            Self::Full => FULL_PROFILE,
            Self::Basic => BASIC_PROFILE,
            Self::Search => SEARCH_PROFILE,
        }
    }

    pub(crate) fn allows(self, tool: &str) -> bool {
        self.tools().contains(&tool)
    }
}

pub fn default_tools() -> Vec<ToolDefinition> {
    tools_for_model()
}

/// Tool definitions for a model: the built-in tools, the `agent` tool with
/// the built-in subagent types, and `update_plan`. The same for every
/// provider.
pub(crate) fn tools_for_model() -> Vec<ToolDefinition> {
    tools_with_settings(&crate::settings::Settings::default())
}

/// As `tools_for_model`, with the tool profile and command time limit the
/// user set.
pub(crate) fn tools_with_settings(settings: &crate::settings::Settings) -> Vec<ToolDefinition> {
    BUILTIN_TOOLS
        .iter()
        .filter(|tool| settings.tools.allows(tool.name()))
        .map(|tool| match tool.definition() {
            _ if tool.name() == "run_command" => {
                run_command::definition(settings.sandbox.max_command_timeout)
            }
            definition => definition,
        })
        // `agent` and `update_plan` need state the stateless `Tool` trait
        // does not carry; the turn loop runs them itself.
        .chain([
            crate::agent::subagent::catalog::SubagentCatalog::builtin().tool_definition(),
            crate::plan::tool_definition(),
        ])
        // Only when the user turned it on and its key is set.
        .chain(
            settings
                .web_search
                .is_some()
                .then(web_search::tool_definition),
        )
        .collect()
}

#[cfg(test)]
pub(crate) fn run_tool_calls(workspace_root: &Path, blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    let mut noop = |_| {};
    run_tool_calls_with_progress(workspace_root, blocks, &mut noop)
}

#[cfg(test)]
pub(crate) fn run_tool_calls_with_progress<F>(
    workspace_root: &Path,
    blocks: &[ContentBlock],
    on_progress: &mut F,
) -> Vec<ContentBlock>
where
    F: FnMut(AgentProgress),
{
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some(run_tool_call_with_progress(
                workspace_root,
                id,
                name,
                input,
                on_progress,
            )),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn run_tool_call(
    workspace_root: &Path,
    tool_use_id: &str,
    name: &str,
    input: &Value,
) -> ContentBlock {
    let mut noop = |_| {};
    run_tool_call_with_progress(workspace_root, tool_use_id, name, input, &mut noop)
}

#[cfg(test)]
pub(crate) fn run_tool_call_with_progress<F>(
    workspace_root: &Path,
    tool_use_id: &str,
    name: &str,
    input: &Value,
    on_progress: &mut F,
) -> ContentBlock
where
    F: FnMut(AgentProgress),
{
    run_tool_call_controlled(
        workspace_root,
        tool_use_id,
        name,
        input,
        &crate::TaskControl::default(),
        on_progress,
    )
}

#[cfg(test)]
pub(crate) fn run_tool_call_controlled<F>(
    workspace_root: &Path,
    tool_use_id: &str,
    name: &str,
    input: &Value,
    control: &crate::TaskControl,
    on_progress: &mut F,
) -> ContentBlock
where
    F: FnMut(AgentProgress),
{
    run_tool_call_checkpointed(
        ToolCall {
            workspace_root,
            tool_use_id,
            name,
            input,
            control,
            settings: &crate::settings::Settings::default(),
            edit_origin: None,
        },
        on_progress,
        &mut |_| Ok(()),
    )
}

/// How a call was authorized, as the turn records it before the call runs.
pub(crate) const HUMAN_APPROVED: &str = "human-approved";
pub(crate) const ALLOWED_BY_POLICY: &str = "allowed-by-policy";
pub(crate) const WORKSPACE_FILE_POLICY: &str = "workspace-file-policy";
pub(crate) const APPROVALS_SKIPPED: &str = "approvals-skipped";

/// One tool call the model asked for, with what it runs against.
#[derive(Clone, Copy)]
pub(crate) struct ToolCall<'a> {
    pub(crate) workspace_root: &'a Path,
    pub(crate) tool_use_id: &'a str,
    pub(crate) name: &'a str,
    pub(crate) input: &'a Value,
    pub(crate) control: &'a crate::TaskControl,
    pub(crate) settings: &'a crate::settings::Settings,
    pub(crate) edit_origin: Option<&'a crate::tools::edit_history::EditOrigin>,
}

pub(crate) fn run_tool_call_checkpointed<F>(
    call: ToolCall<'_>,
    on_progress: &mut F,
    before_execute: &mut dyn FnMut(&str) -> std::io::Result<()>,
) -> ContentBlock
where
    F: FnMut(AgentProgress),
{
    let ToolCall {
        workspace_root,
        tool_use_id,
        name,
        input,
        control,
        settings,
        edit_origin,
    } = call;
    on_progress(AgentProgress::ToolCallStarted {
        name: name.to_string(),
    });
    let result = (|| -> Result<String, ToolError> {
        if control.is_cancelled() {
            return Err("Cancelled before execution".into());
        }
        let gate = crate::approval::gate(name, input, workspace_root, settings, &|grant| {
            control.is_granted(grant)
        })?;
        let mut authorization = WORKSPACE_FILE_POLICY;
        let restrictions = match gate {
            crate::approval::Gate::Deny(reason) => return Err(reason.into()),
            crate::approval::Gate::Run(restrictions) => {
                if settings.approval.mode == crate::settings::ApprovalMode::Bypass {
                    authorization = APPROVALS_SKIPPED;
                } else if restrictions.is_some() {
                    authorization = ALLOWED_BY_POLICY;
                }
                restrictions
            }
            crate::approval::Gate::Ask(ask) => {
                let permissions = match name {
                    "git_commit" => {
                        "Linux sandbox: workspace and Git metadata writable; agent state hidden; network denied"
                    }
                    "run_command" => {
                        "Linux sandbox: workspace writable; existing Git metadata read-only; agent state hidden; network denied"
                    }
                    _ => "Socium's file tools inside the workspace; nothing runs",
                };
                let request = serde_json::json!({"workspace":workspace_root,"tool_call_id":tool_use_id,"arguments":input,"review":ask.review,"permissions":permissions,"reason":ask.reason});
                if !control.is_interactive() {
                    return Err(format!(
                        "Needs approval, but no one can answer here: {} Allow it with an approval rule in Socium's config.toml, or run it from the TUI.",
                        ask.reason
                    )
                    .into());
                }
                if !control.approve(name, &request, ask.grant, on_progress) {
                    return Err("Denied by the user. Do not retry the same call; ask the user how to proceed if it is needed.".into());
                }
                if let Some(review) = &ask.review
                    && name == "git_commit"
                    && git::commit_review(workspace_root, input)? != *review
                {
                    return Err(
                        "Repository changed during approval; commit refused, request a new review"
                            .into(),
                    );
                }
                if let Some(review) = &ask.review
                    && name != "git_commit"
                    && crate::approval::edit_review(name, input, workspace_root) != *review
                {
                    return Err(
                        "The file changed during approval; edit refused, request it again".into(),
                    );
                }
                authorization = HUMAN_APPROVED;
                None
            }
        };
        before_execute(authorization).map_err(|e| {
            format!("Could not persist execution intent; tool was not started: {e}")
        })?;
        if control.is_cancelled() {
            return Err("Cancelled before execution; no side effects".into());
        }
        match BUILTIN_TOOLS.iter().find(|tool| tool.name() == name) {
            Some(tool) => tool.run(
                &ToolContext {
                    workspace_root,
                    control,
                    settings,
                    restrictions: restrictions.as_ref(),
                    edit_origin,
                },
                input,
                on_progress,
            ),
            None => Err(ToolError::UnsupportedTool(name.into())),
        }
    })();
    let is_error = result.is_err();
    on_progress(AgentProgress::ToolCallFinished {
        name: name.to_string(),
        is_error,
    });

    match result {
        Ok(content) => ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content,
            is_error: None,
        },
        Err(err) => ContentBlock::ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: err.to_string(),
            is_error: Some(true),
        },
    }
}

/// [`run_tool_call_checkpointed`] on a blocking thread, so the runtime keeps
/// polling everything else, such as the other subagents of a batch, while
/// the tool works. `on_progress` and `before_execute` still run on the
/// awaiting task, in the order the call makes them.
pub(crate) async fn run_tool_call_on_blocking_thread(
    call: ToolCall<'_>,
    on_progress: &mut (dyn FnMut(AgentProgress) + Send),
    before_execute: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
) -> ContentBlock {
    enum Relayed {
        Progress(AgentProgress),
        BeforeExecute(String, tokio::sync::oneshot::Sender<std::io::Result<()>>),
    }
    let (relay, mut relayed) = tokio::sync::mpsc::unbounded_channel();
    let (root, id, name, input) = (
        call.workspace_root.to_path_buf(),
        call.tool_use_id.to_string(),
        call.name.to_string(),
        call.input.clone(),
    );
    let (control, settings, edit_origin) = (
        call.control.clone(),
        call.settings.clone(),
        call.edit_origin.cloned(),
    );
    let call = tokio::task::spawn_blocking(move || {
        let stopped = || std::io::Error::other("the turn stopped waiting for this call");
        run_tool_call_checkpointed(
            ToolCall {
                workspace_root: &root,
                tool_use_id: &id,
                name: &name,
                input: &input,
                control: &control,
                settings: &settings,
                edit_origin: edit_origin.as_ref(),
            },
            &mut |event| {
                let _ = relay.send(Relayed::Progress(event));
            },
            &mut |authorization| {
                let (reply, answer) = tokio::sync::oneshot::channel();
                relay
                    .send(Relayed::BeforeExecute(authorization.to_string(), reply))
                    .map_err(|_| stopped())?;
                answer.blocking_recv().map_err(|_| stopped())?
            },
        )
    });
    while let Some(relayed) = relayed.recv().await {
        match relayed {
            Relayed::Progress(event) => on_progress(event),
            Relayed::BeforeExecute(authorization, reply) => {
                let _ = reply.send(before_execute(&authorization));
            }
        }
    }
    call.await
        .unwrap_or_else(|err| std::panic::resume_unwind(err.into_panic()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_temp_dir;
    use serde_json::json;
    use std::fs;

    fn offered(config: &str) -> Vec<String> {
        let (settings, _) = crate::settings::Settings::from_toml(config, None).unwrap();
        tools_with_settings(&settings)
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    #[test]
    fn the_default_profile_offers_every_built_in_tool() {
        let mut expected = BUILTIN_TOOLS
            .iter()
            .map(|tool| tool.name().to_string())
            .collect::<Vec<_>>();
        assert_eq!(expected.len(), 12);
        expected.extend(["agent".to_string(), "update_plan".to_string()]);
        assert_eq!(offered(""), expected);
        assert_eq!(offered("[tools]\nprofile = \"full\""), expected);
    }

    #[test]
    fn a_tool_profile_offers_only_its_built_in_tools() {
        assert_eq!(
            offered("[tools]\nprofile = \"basic\""),
            [
                "read_file",
                "apply_patch",
                "git_commit",
                "run_command",
                "str_replace_based_edit_tool",
                "agent",
                "update_plan",
            ]
        );
        assert_eq!(
            offered("[tools]\nprofile = \"search\""),
            [
                "read_file",
                "search_text",
                "apply_patch",
                "git_commit",
                "find_symbol",
                "run_command",
                "str_replace_based_edit_tool",
                "agent",
                "update_plan",
            ]
        );
    }

    #[test]
    fn run_tool_call_returns_success_for_list_files_without_path() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("one.txt"), "1").unwrap();

        let result = run_tool_call(workspace.path(), "tool_4", "list_files", &json!({}));

        match result {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_4");
                assert!(content.contains("Path: ."));
                assert!(content.contains("one.txt"));
                assert_eq!(is_error, None);
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_tool_call_returns_error_for_unsupported_tool() {
        let workspace = make_temp_dir("workspace");
        let result = run_tool_call(workspace.path(), "tool_1", "unknown_tool", &json!({}));

        match result {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_1");
                assert!(content.contains("Unsupported tool"));
                assert_eq!(is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_tool_call_returns_success_for_read_file() {
        let workspace = make_temp_dir("workspace");
        let file_path = workspace.path().join("ok.txt");
        fs::write(&file_path, "ok").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_3",
            "read_file",
            &json!({"path": "ok.txt"}),
        );

        match result {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_3");
                assert!(content.contains("Path: ok.txt"));
                assert!(content.contains("ok"));
                assert_eq!(is_error, None);
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_tool_call_returns_success_for_text_editor_view() {
        let workspace = make_temp_dir("workspace");
        fs::write(workspace.path().join("edit.txt"), "first\nsecond\nthird\n").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_te_1",
            TEXT_EDITOR_TOOL_NAME,
            &json!({"command": "view", "path": "edit.txt", "view_range": [2, -1]}),
        );

        match result {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_te_1");
                assert!(content.contains("Path: edit.txt"));
                assert!(content.contains("     2\tsecond\n     3\tthird"));
                assert_eq!(is_error, None);
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_tool_calls_returns_error_for_invalid_read_file_input() {
        let workspace = make_temp_dir("workspace");
        let blocks = vec![ContentBlock::ToolUse {
            id: "tool_2".to_string(),
            name: "read_file".to_string(),
            input: json!({}),
        }];

        let results = run_tool_calls(workspace.path(), &blocks);

        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_2");
                assert!(content.contains("read_file requires a string field: path"));
                assert_eq!(*is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_tool_calls_returns_error_for_invalid_list_files_input() {
        let workspace = make_temp_dir("workspace");
        let blocks = vec![ContentBlock::ToolUse {
            id: "tool_5".to_string(),
            name: "list_files".to_string(),
            input: json!({"path": 1}),
        }];

        let results = run_tool_calls(workspace.path(), &blocks);

        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_5");
                assert!(content.contains("list_files requires an optional string field: path"));
                assert_eq!(*is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn null_optional_fields_take_their_defaults() {
        let workspace = make_temp_dir("workspace");
        std::fs::write(workspace.path().join("a.txt"), "hello\n").unwrap();
        let blocks = [
            (
                "list_files",
                json!({"path": null, "recursive": null, "glob": null}),
            ),
            (
                "search_text",
                json!({"pattern": "hello", "path": null, "glob": null, "max_results": null}),
            ),
            ("git_log", json!({"path": null, "max_count": null})),
        ]
        .into_iter()
        .map(|(name, input)| ContentBlock::ToolUse {
            id: name.to_string(),
            name: name.to_string(),
            input,
        })
        .collect::<Vec<_>>();

        let results = run_tool_calls(workspace.path(), &blocks);

        for result in &results {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } = result
            else {
                panic!("expected tool_result");
            };
            assert!(
                !content.contains("requires"),
                "{tool_use_id}: {content} ({is_error:?})"
            );
        }
        assert!(matches!(
            &results[0],
            ContentBlock::ToolResult { content, is_error: None, .. } if content.contains("a.txt")
        ));
        assert!(matches!(
            &results[1],
            ContentBlock::ToolResult { content, is_error: None, .. } if content.contains("a.txt")
        ));
    }

    #[test]
    fn run_tool_calls_returns_error_for_non_object_list_files_input() {
        let workspace = make_temp_dir("workspace");
        let blocks = vec![ContentBlock::ToolUse {
            id: "tool_6".to_string(),
            name: "list_files".to_string(),
            input: json!(["not", "an", "object"]),
        }];

        let results = run_tool_calls(workspace.path(), &blocks);

        assert_eq!(results.len(), 1);
        match &results[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tool_6");
                assert!(content.contains("list_files requires an object input"));
                assert_eq!(*is_error, Some(true));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn the_text_editor_is_a_schema_tool_every_provider_can_send() {
        let editor = tools_for_model()
            .into_iter()
            .find(|tool| tool.name == TEXT_EDITOR_TOOL_NAME)
            .expect("text editor");
        assert_eq!(editor.input_schema["required"], json!(["command", "path"]));
        assert!(editor.input_schema["properties"]["insert_text"].is_object());
    }
}
