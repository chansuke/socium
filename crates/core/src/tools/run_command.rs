use crate::AgentProgress;
use crate::constants::*;
use crate::tools::error::ToolError;
use crate::tools::input::{optional_string_field, optional_timeout_ms, require_string_array_field};
use crate::tools::text_file::truncate_by_characters;
use crate::tools::workspace::resolve_workspace_command_cwd;
use crate::tools::{CustomToolDefinition, Tool, ToolContext, ToolDefinition};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

pub(crate) struct RunCommandTool;

impl Tool for RunCommandTool {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::Custom(CustomToolDefinition {
            name: "run_command".to_string(),
            description: "Run a workspace command for search, build, test, lint, or local execution. Commands are passed as an array of executable plus arguments, not as a shell string. Commands run with host-user authority after explicit human approval through the application. No filesystem or network sandbox is implied."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "array",
                        "description": "Executable followed by arguments, each as a separate string.",
                        "items": {
                            "type": "string"
                        },
                        "minItems": 1
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Optional relative working directory inside the workspace. Defaults to the workspace root."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": format!("Optional timeout in milliseconds. Must be between 1 and {MAX_COMMAND_TIMEOUT_MS}.")
                    }

                },
                "required": ["command"],
                "additionalProperties": false
            }),
        })
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        let mut forward = |progress| on_progress(progress);
        run_command_controlled(ctx.workspace_root, input, ctx.control, &mut forward)
    }
}

struct CommandExecution {
    artifacts: String,
    stop_reason: String,
    command_display: String,
    cwd_display: String,
    timeout_ms: u64,
    timed_out: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

struct RunCommandInput<'a> {
    command: Vec<String>,
    cwd: &'a str,
    timeout_ms: u64,
}

impl<'a> RunCommandInput<'a> {
    fn parse(input: &'a Value) -> Result<Self, ToolError> {
        let input_object = input
            .as_object()
            .ok_or("run_command requires an object input")?;
        let command = require_string_array_field(input_object, "command", "run_command")?;
        let cwd = optional_string_field(input_object, "cwd", "run_command cwd")?.unwrap_or(".");
        let timeout_ms = optional_timeout_ms(input_object)?;
        Ok(Self {
            command,
            cwd,
            timeout_ms,
        })
    }
}

#[cfg(test)]
pub(crate) fn run_command_from_workspace_with_progress<F>(
    workspace_root: &Path,
    input: &Value,
    on_progress: &mut F,
) -> Result<String, ToolError>
where
    F: FnMut(AgentProgress),
{
    run_command_controlled(
        workspace_root,
        input,
        &crate::TaskControl::default(),
        on_progress,
    )
}

fn run_command_controlled<F>(
    workspace_root: &Path,
    input: &Value,
    control: &crate::TaskControl,
    on_progress: &mut F,
) -> Result<String, ToolError>
where
    F: FnMut(AgentProgress),
{
    let RunCommandInput {
        command,
        cwd,
        timeout_ms,
    } = RunCommandInput::parse(input)?;
    let resolved_cwd = resolve_workspace_command_cwd(workspace_root, cwd)?;
    on_progress(AgentProgress::CommandStarted {
        command: display_command(&command),
        cwd: cwd.to_string(),
    });
    let execution = execute_command(
        &command,
        &resolved_cwd,
        cwd,
        timeout_ms,
        control,
        workspace_root,
        on_progress,
    )?;
    on_progress(AgentProgress::CommandFinished {
        command: execution.command_display.clone(),
        exit_code: execution.exit_code,
        timed_out: execution.timed_out,
        stdout_preview: format_command_stream(&execution.stdout)
            .trim_end_matches('\n')
            .to_string(),
        stderr_preview: format_command_stream(&execution.stderr)
            .trim_end_matches('\n')
            .to_string(),
    });

    let output = format_command_execution(&execution);
    if control.is_cancelled() || execution.timed_out || execution.exit_code != Some(0) {
        Err(output.into())
    } else {
        Ok(output)
    }
}

fn execute_command(
    command: &[String],
    cwd: &Path,
    cwd_display: &str,
    timeout_ms: u64,
    control: &crate::TaskControl,
    workspace_root: &Path,
    on_progress: &mut dyn FnMut(AgentProgress),
) -> Result<CommandExecution, String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let stem = format!(
        ".socium/outputs/{}-{timestamp}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let open = |suffix: &str| -> Result<std::fs::File, String> {
        let path = crate::agent::state::workspace_state_path(
            workspace_root,
            &format!("{}.{suffix}", stem.strip_prefix(".socium/").unwrap()),
        )
        .map_err(|e| e.to_string())?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        opts.open(path).map_err(|e| e.to_string())
    };
    let mut files = [open("stdout")?, open("stderr")?];
    let mut totals = [0u64; 2];
    let mut reported = std::time::Instant::now();
    const ARTIFACT_LIMIT: u64 = 8 * 1024 * 1024;
    let mut observer = |stderr: bool, bytes: &[u8]| -> std::io::Result<()> {
        let index = usize::from(stderr);
        let keep = (ARTIFACT_LIMIT.saturating_sub(totals[index]) as usize).min(bytes.len());
        files[index].write_all(&bytes[..keep])?;
        totals[index] = totals[index].saturating_add(bytes.len() as u64);
        if reported.elapsed() >= Duration::from_millis(100) {
            on_progress(AgentProgress::CommandOutput {
                stdout_bytes: totals[0],
                stderr_bytes: totals[1],
            });
            reported = std::time::Instant::now();
        }
        Ok(())
    };
    let output=crate::process::run_observed(command,cwd,Duration::from_millis(timeout_ms),control,&mut observer)
        .map_err(|e|format!("Command failed or output capture failed: {e}; partial artifacts: {stem}.stdout / {stem}.stderr"))?;
    for file in &files {
        file.sync_all()
            .map_err(|e| format!("Command completed but output sync failed: {e}"))?;
    }
    let artifacts = format!(
        "{stem}.stdout ({} bytes), {stem}.stderr ({} bytes); artifact cap 8 MiB per stream, truncated={}",
        totals[0],
        totals[1],
        totals.iter().any(|n| *n > ARTIFACT_LIMIT)
    );
    let stop_reason = if output.cancelled {
        "cancelled".into()
    } else if output.timed_out {
        "timeout".into()
    } else if let Some(code) = output.status.code() {
        format!("exit {code}")
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            format!("signal {:?}", output.status.signal())
        }
        #[cfg(not(unix))]
        {
            "terminated".into()
        }
    };
    let timed_out = output.timed_out;
    let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.truncated {
        stdout.push_str("\n(output capture truncated at 256 KiB per stream)");
    }
    if output.cancelled {
        stdout.push_str("\n(command cancelled)");
    }

    Ok(CommandExecution {
        artifacts,
        stop_reason,
        command_display: display_command(command),
        cwd_display: cwd_display.to_string(),
        timeout_ms,
        timed_out,
        exit_code: output.status.code(),
        stdout,
        stderr,
    })
}

fn format_command_execution(execution: &CommandExecution) -> String {
    let stdout = format_command_stream(&execution.stdout);
    let stderr = format_command_stream(&execution.stderr);
    let exit_code = match execution.exit_code {
        Some(code) => code.to_string(),
        None => "terminated".to_string(),
    };

    format!(
        "Command: {}\nCwd: {}\nPermission: approved host command\nStop reason: {}\nArtifacts: {}\nTimeout: {} ms\nTimed out: {}\nExit code: {}\nStdout:\n{}\nStderr:\n{}",
        execution.command_display,
        execution.cwd_display,
        execution.stop_reason,
        execution.artifacts,
        execution.timeout_ms,
        execution.timed_out,
        exit_code,
        stdout,
        stderr
    )
}

fn format_command_stream(stream: &str) -> String {
    if stream.is_empty() {
        return "(empty)".to_string();
    }

    let (truncated, was_truncated) = truncate_by_characters(stream, MAX_COMMAND_OUTPUT_CHARACTERS);
    if was_truncated {
        format!(
            "{truncated}\n(truncated: showing first {} characters)",
            MAX_COMMAND_OUTPUT_CHARACTERS
        )
    } else {
        truncated
    }
}

fn display_command(command: &[String]) -> String {
    command
        .iter()
        .map(|part| {
            if part.is_empty() || part.chars().any(char::is_whitespace) {
                format!("{part:?}")
            } else {
                part.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TaskControl, agent::types::ContentBlock, tools::run_tool_call_controlled};
    #[test]
    fn forged_approval_is_denied_and_human_approval_is_single_use() {
        let workspace = tempfile::tempdir().unwrap();
        let input = json!({"command":["sh","-c","echo ok > marker"],"approval":"destructive"});
        let control = TaskControl::default();
        let result = run_tool_call_controlled(
            workspace.path(),
            "one",
            "run_command",
            &input,
            &control,
            &mut |_| {},
        );
        assert!(matches!(
            result,
            ContentBlock::ToolResult {
                is_error: Some(true),
                ..
            }
        ));
        assert!(!workspace.path().join("marker").exists());
        control.enable_interactive_approval();
        let mut id = None;
        let result = run_tool_call_controlled(
            workspace.path(),
            "two",
            "run_command",
            &input,
            &control,
            &mut |event| {
                if let AgentProgress::ApprovalRequested { request_id, .. } = event {
                    id = Some(request_id);
                    assert!(control.answer_approval(request_id, true));
                }
            },
        );
        assert!(matches!(
            result,
            ContentBlock::ToolResult { is_error: None, .. }
        ));
        assert!(workspace.path().join("marker").exists());
        assert!(!control.answer_approval(id.unwrap(), true));
    }
    #[test]
    fn signal_termination_is_a_tool_error() {
        let workspace = tempfile::tempdir().unwrap();
        assert!(
            run_command_from_workspace_with_progress(
                workspace.path(),
                &json!({"command":["sh","-c","kill -TERM $$"]}),
                &mut |_| {}
            )
            .is_err()
        );
    }
}
