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

#[cfg(test)]
use crate::conversation::ContentBlock;
use crate::progress::AgentProgress;
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
    pub(crate) restrictions: Option<&'a crate::access::Restrictions>,
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

/// Tool definitions for a model with the tool profile and command time
/// limit the user set: the built-in tools, then `agent` as the caller's
/// subagent catalog defines it, and `update_plan`. The same for every
/// provider.
pub(crate) fn tools_with_settings(
    settings: &crate::settings::Settings,
    agent: ToolDefinition,
) -> Vec<ToolDefinition> {
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
        .chain([agent, crate::plan::tool_definition()])
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
    crate::dispatch::run_tool_call_checkpointed(
        crate::dispatch::ToolCall {
            workspace_root,
            tool_use_id,
            name,
            input,
            control,
            settings: &crate::settings::Settings::default(),
            edit_origin: None,
            mcp: None,
        },
        on_progress,
        &mut |_| Ok(()),
    )
}

/// The built-in tool called `name`, if there is one.
pub(crate) fn builtin(name: &str) -> Option<&'static dyn Tool> {
    BUILTIN_TOOLS
        .iter()
        .copied()
        .find(|tool| tool.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tools_for_model;
    use crate::constants::TEXT_EDITOR_TOOL_NAME;
    use crate::test_support::make_temp_dir;
    use serde_json::json;
    use std::fs;

    fn offered(config: &str) -> Vec<String> {
        let (settings, _) = crate::settings::Settings::from_toml(config, None).unwrap();
        tools_with_settings(
            &settings,
            crate::agent::subagent::catalog::SubagentCatalog::builtin().tool_definition(),
        )
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

    fn run_mcp(
        registry: &std::sync::Arc<crate::mcp::McpRegistry>,
        name: &str,
        control: &crate::TaskControl,
        on_progress: &mut dyn FnMut(AgentProgress),
    ) -> (Vec<String>, String, Option<bool>) {
        let settings = crate::settings::Settings::default();
        run_mcp_with(registry, name, control, &settings, on_progress)
    }

    fn run_mcp_with(
        registry: &std::sync::Arc<crate::mcp::McpRegistry>,
        name: &str,
        control: &crate::TaskControl,
        settings: &crate::settings::Settings,
        on_progress: &mut dyn FnMut(AgentProgress),
    ) -> (Vec<String>, String, Option<bool>) {
        let workspace = make_temp_dir("mcp_call");
        let mut authorizations = Vec::new();
        let result = crate::dispatch::run_tool_call_checkpointed(
            crate::dispatch::ToolCall {
                workspace_root: workspace.path(),
                tool_use_id: "call",
                name,
                input: &json!({"text": "hi"}),
                control,
                settings,
                edit_origin: None,
                mcp: Some(registry),
            },
            &mut |event| on_progress(event),
            &mut |authorization| {
                authorizations.push(authorization.to_string());
                Ok(())
            },
        );
        let ContentBlock::ToolResult {
            content, is_error, ..
        } = result
        else {
            panic!("expected tool_result");
        };
        (authorizations, content, is_error)
    }

    #[test]
    fn an_mcp_tool_runs_when_allowed_and_asks_otherwise() {
        use crate::mcp::tests::{connect_fake, fake_settings};
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &["echo"])]);
        let registry = std::sync::Arc::new(registry);
        let headless = crate::TaskControl::default();

        let (authorizations, content, is_error) =
            run_mcp(&registry, "mcp__fake__echo", &headless, &mut |_| {});
        assert_eq!((content.as_str(), is_error), ("echo: hi", None));
        assert_eq!(authorizations, [crate::constants::ALLOWED_BY_POLICY]);

        let (authorizations, content, is_error) =
            run_mcp(&registry, "mcp__fake__image", &headless, &mut |_| {});
        assert_eq!(is_error, Some(true));
        assert!(authorizations.is_empty(), "nothing may start unapproved");
        assert_eq!(
            content,
            "Needs approval, but no one can answer here: MCP tools run in the fake server on the host, outside Socium's sandbox. Allow it by adding \"image\" to allow under [mcp.servers.fake] in Socium's config.toml, or run it from the TUI."
        );

        // Allowed for the session, it runs unasked from then on.
        let interactive = crate::TaskControl::default();
        interactive.enable_interactive_approval();
        let answerer = interactive.clone();
        let mut asked = Vec::new();
        let mut answer = |event: AgentProgress| {
            if let AgentProgress::ApprovalRequested {
                request_id,
                input,
                session_option,
                ..
            } = event
            {
                asked.push((input, session_option));
                answerer.answer(request_id, crate::ApprovalAnswer::AllowForSession);
            }
        };
        let (authorizations, content, _) =
            run_mcp(&registry, "mcp__fake__image", &interactive, &mut answer);
        assert_eq!(content, "a chart\n[image content omitted]");
        assert_eq!(authorizations, [crate::constants::HUMAN_APPROVED]);
        run_mcp(&registry, "mcp__fake__image", &interactive, &mut answer);
        assert_eq!(asked.len(), 1, "{asked:?}");
        let (details, session_option) = &asked[0];
        assert!(
            details.contains(
                "Permissions: MCP tools run in the fake server on the host, outside Socium's sandbox."
            ),
            "{details}"
        );
        assert_eq!(
            session_option.as_deref(),
            Some("the MCP tool `mcp__fake__image`")
        );
    }

    /// ADR 20: `--dangerously-skip-permissions` runs an MCP tool of a host
    /// server without asking too, allowed or not, and records it as such;
    /// `auto` still asks, since it covers sandboxed commands only.
    #[test]
    fn skipping_approvals_runs_an_mcp_tool_without_asking() {
        use crate::mcp::tests::{connect_fake, fake_settings};
        use crate::settings::ApprovalMode;
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &["echo"])]);
        let registry = std::sync::Arc::new(registry);
        let headless = crate::TaskControl::default();
        let mut settings = crate::settings::Settings::default();

        settings.approval.mode = ApprovalMode::Bypass;
        for (name, expected) in [
            ("mcp__fake__echo", "echo: hi"),
            ("mcp__fake__image", "a chart\n[image content omitted]"),
        ] {
            let (authorizations, content, is_error) =
                run_mcp_with(&registry, name, &headless, &settings, &mut |_| {});
            assert_eq!((content.as_str(), is_error), (expected, None), "{name}");
            assert_eq!(
                authorizations,
                [crate::constants::APPROVALS_SKIPPED],
                "{name}"
            );
        }

        for mode in [ApprovalMode::Ask, ApprovalMode::Auto] {
            settings.approval.mode = mode;
            let (authorizations, content, is_error) = run_mcp_with(
                &registry,
                "mcp__fake__image",
                &headless,
                &settings,
                &mut |_| {},
            );
            assert_eq!(is_error, Some(true), "{mode:?}");
            assert!(authorizations.is_empty(), "{mode:?}");
            assert!(content.starts_with("Needs approval"), "{mode:?}: {content}");
            let (authorizations, _, is_error) = run_mcp_with(
                &registry,
                "mcp__fake__echo",
                &headless,
                &settings,
                &mut |_| {},
            );
            assert_eq!(is_error, None, "{mode:?}");
            assert_eq!(
                authorizations,
                [crate::constants::ALLOWED_BY_POLICY],
                "{mode:?}"
            );
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
