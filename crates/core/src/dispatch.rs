//! Runs one tool call the model asked for: through the approval gate, the
//! caller's checkpoint, and then the tool. It sits above `tools` and
//! `approval` so that neither needs the other's decisions.
use crate::conversation::ContentBlock;
use crate::progress::AgentProgress;
use crate::tool_error::ToolError;
use serde_json::Value;
use std::path::Path;

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
        let mut authorization = crate::constants::WORKSPACE_FILE_POLICY;
        let restrictions = match gate {
            crate::approval::Gate::Deny(reason) => return Err(reason.into()),
            crate::approval::Gate::Run(restrictions) => {
                if settings.approval.mode == crate::settings::ApprovalMode::Bypass {
                    authorization = crate::constants::APPROVALS_SKIPPED;
                } else if restrictions.is_some() {
                    authorization = crate::constants::ALLOWED_BY_POLICY;
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
                    && crate::tools::git::commit_review(workspace_root, input)? != *review
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
                authorization = crate::constants::HUMAN_APPROVED;
                None
            }
        };
        before_execute(authorization).map_err(|e| {
            format!("Could not persist execution intent; tool was not started: {e}")
        })?;
        if control.is_cancelled() {
            return Err("Cancelled before execution; no side effects".into());
        }
        match crate::tools::builtin(name) {
            Some(tool) => tool.run(
                &crate::tools::ToolContext {
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
