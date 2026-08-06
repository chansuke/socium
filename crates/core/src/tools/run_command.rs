use crate::AgentProgress;
use crate::constants::*;
use crate::tools::error::ToolError;
use crate::tools::input::{optional_string_field, optional_timeout_ms, require_string_array_field};
use crate::tools::text_file::truncate_by_characters;
use crate::tools::workspace::resolve_workspace_command_cwd;
use crate::tools::{CustomToolDefinition, Tool, ToolContext, ToolDefinition};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) struct RunCommandTool;

impl Tool for RunCommandTool {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::Custom(CustomToolDefinition {
            name: "run_command".to_string(),
            description: "Run a workspace command for search, build, test, lint, or local execution. Commands are passed as an array of executable plus arguments, not as a shell string. The runtime classifies commands as safe-read-only, workspace-write, or destructive and may require the approval field to match the required permission level."
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
                    },
                    "approval": {
                        "type": "string",
                        "description": "Optional approval granted by the user when the runtime requires it.",
                        "enum": ["workspace-write", "destructive"]
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
        run_command_from_workspace_with_progress(ctx.workspace_root, input, &mut forward)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum CommandApproval {
    None,
    WorkspaceWrite,
    Destructive,
}

impl CommandApproval {
    fn parse(input: Option<&str>) -> Result<Self, String> {
        match input {
            None => Ok(Self::None),
            Some("workspace-write") => Ok(Self::WorkspaceWrite),
            Some("destructive") => Ok(Self::Destructive),
            Some(other) => Err(format!(
                "run_command approval must be either 'workspace-write' or 'destructive', got '{other}'"
            )),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::WorkspaceWrite => "workspace-write",
            Self::Destructive => "destructive",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum CommandPermission {
    SafeReadOnly,
    WorkspaceWrite,
    Destructive,
}

impl CommandPermission {
    fn label(self) -> &'static str {
        match self {
            Self::SafeReadOnly => "safe-read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::Destructive => "destructive",
        }
    }

    fn required_approval(self) -> CommandApproval {
        match self {
            Self::SafeReadOnly => CommandApproval::None,
            Self::WorkspaceWrite => CommandApproval::WorkspaceWrite,
            Self::Destructive => CommandApproval::Destructive,
        }
    }
}

struct CommandExecution {
    permission: CommandPermission,
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
    approval: CommandApproval,
}

impl<'a> RunCommandInput<'a> {
    fn parse(input: &'a Value) -> Result<Self, ToolError> {
        let input_object = input
            .as_object()
            .ok_or("run_command requires an object input")?;
        let command = require_string_array_field(input_object, "command", "run_command")?;
        let cwd = optional_string_field(input_object, "cwd", "run_command cwd")?.unwrap_or(".");
        let timeout_ms = optional_timeout_ms(input_object)?;
        let approval = CommandApproval::parse(optional_string_field(
            input_object,
            "approval",
            "run_command approval",
        )?)?;
        Ok(Self {
            command,
            cwd,
            timeout_ms,
            approval,
        })
    }
}

pub(crate) fn run_command_from_workspace_with_progress<F>(
    workspace_root: &Path,
    input: &Value,
    on_progress: &mut F,
) -> Result<String, ToolError>
where
    F: FnMut(AgentProgress),
{
    let RunCommandInput {
        command,
        cwd,
        timeout_ms,
        approval,
    } = RunCommandInput::parse(input)?;
    let permission = classify_command(&command);
    let required_approval = permission.required_approval();

    if approval < required_approval {
        on_progress(AgentProgress::ApprovalRequired {
            tool: "run_command".to_string(),
            reason: format!("{} approval required", required_approval.label()),
        });
        return Err(format!(
            "Command '{}' is classified as {} and requires {} approval. Rerun with approval set to '{}'.",
            display_command(&command),
            permission.label(),
            required_approval.label(),
            required_approval.label()
        ).into());
    }

    let resolved_cwd = resolve_workspace_command_cwd(workspace_root, cwd)?;
    on_progress(AgentProgress::CommandStarted {
        command: display_command(&command),
        cwd: cwd.to_string(),
    });
    let execution = execute_command(&command, &resolved_cwd, cwd, timeout_ms, permission)?;
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
    if execution.timed_out || execution.exit_code.unwrap_or_default() != 0 {
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
    permission: CommandPermission,
) -> Result<CommandExecution, String> {
    let program = command
        .first()
        .ok_or("run_command command must contain at least one element")?;
    let args = &command[1..];

    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "Failed to spawn command '{}': {e}",
                display_command(command)
            )
        })?;

    let timeout = Duration::from_millis(timeout_ms);
    let start = Instant::now();
    let mut timed_out = false;

    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    child.kill().map_err(|e| {
                        format!(
                            "Failed to terminate timed out command '{}': {e}",
                            display_command(command)
                        )
                    })?;
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                return Err(format!(
                    "Failed while waiting for command '{}': {e}",
                    display_command(command)
                ));
            }
        }
    }

    let output = child.wait_with_output().map_err(|e| {
        format!(
            "Failed to collect output for command '{}': {e}",
            display_command(command)
        )
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    Ok(CommandExecution {
        permission,
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
        "Command: {}\nCwd: {}\nPermission: {}\nTimeout: {} ms\nTimed out: {}\nExit code: {}\nStdout:\n{}\nStderr:\n{}",
        execution.command_display,
        execution.cwd_display,
        execution.permission.label(),
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

fn classify_command(command: &[String]) -> CommandPermission {
    let Some(program) = command.first() else {
        return CommandPermission::Destructive;
    };
    let program_name = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program.as_str());
    let first_arg = command.get(1).map(String::as_str).unwrap_or_default();

    if is_destructive_command(program_name, first_arg) {
        CommandPermission::Destructive
    } else if is_safe_read_only_command(program_name, first_arg) {
        CommandPermission::SafeReadOnly
    } else {
        CommandPermission::WorkspaceWrite
    }
}

fn is_safe_read_only_command(program: &str, first_arg: &str) -> bool {
    matches!(
        program,
        "cat"
            | "echo"
            | "env"
            | "find"
            | "git"
            | "grep"
            | "head"
            | "ls"
            | "pwd"
            | "rg"
            | "sed"
            | "sort"
            | "stat"
            | "tail"
            | "uname"
            | "wc"
            | "which"
    ) && (program != "git"
        || matches!(
            first_arg,
            "branch" | "diff" | "log" | "rev-parse" | "show" | "status"
        ))
}

fn is_destructive_command(program: &str, first_arg: &str) -> bool {
    matches!(
        program,
        "chmod" | "chown" | "dd" | "fdisk" | "mkfs" | "mv" | "rm" | "rmdir"
    ) || (program == "git"
        && matches!(
            first_arg,
            "checkout"
                | "cherry-pick"
                | "clean"
                | "merge"
                | "rebase"
                | "reset"
                | "restore"
                | "revert"
                | "switch"
        ))
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
    use crate::agent::types::ContentBlock;
    use crate::test_support::make_temp_dir;
    use crate::tools::{run_tool_call, run_tool_call_with_progress};
    use serde_json::json;
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn run_command_runs_safe_command_in_requested_cwd() {
        let workspace = make_temp_dir("workspace");
        let nested = workspace.path().join("nested");
        fs::create_dir(&nested).unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_cmd_pwd",
            "run_command",
            &json!({
                "command": ["pwd"],
                "cwd": "nested"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Command: pwd"));
                assert!(content.contains("Cwd: nested"));
                assert!(content.contains("Permission: safe-read-only"));
                assert!(content.contains("Exit code: 0"));
                assert!(content.contains(nested.to_string_lossy().as_ref()));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_command_emits_tool_and_command_progress_events() {
        let workspace = make_temp_dir("workspace");
        let mut progress = Vec::new();

        let result = run_tool_call_with_progress(
            workspace.path(),
            "tool_cmd_progress",
            "run_command",
            &json!({
                "command": ["pwd"]
            }),
            &mut |event| progress.push(event),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, None),
            _ => panic!("expected tool_result"),
        }
        assert_eq!(
            progress,
            vec![
                AgentProgress::ToolCallStarted {
                    name: "run_command".to_string(),
                },
                AgentProgress::CommandStarted {
                    command: "pwd".to_string(),
                    cwd: ".".to_string(),
                },
                AgentProgress::CommandFinished {
                    command: "pwd".to_string(),
                    exit_code: Some(0),
                    timed_out: false,
                    stdout_preview: workspace.path().to_string_lossy().trim().to_string(),
                    stderr_preview: "(empty)".to_string(),
                },
                AgentProgress::ToolCallFinished {
                    name: "run_command".to_string(),
                    is_error: false,
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_command_emits_approval_required_event_when_permission_is_missing() {
        let workspace = make_temp_dir("workspace");
        let mut progress = Vec::new();

        let result = run_tool_call_with_progress(
            workspace.path(),
            "tool_cmd_needs_approval",
            "run_command",
            &json!({
                "command": ["touch", "created.txt"]
            }),
            &mut |event| progress.push(event),
        );

        match result {
            ContentBlock::ToolResult { is_error, .. } => assert_eq!(is_error, Some(true)),
            _ => panic!("expected tool_result"),
        }
        assert_eq!(
            progress,
            vec![
                AgentProgress::ToolCallStarted {
                    name: "run_command".to_string(),
                },
                AgentProgress::ApprovalRequired {
                    tool: "run_command".to_string(),
                    reason: "workspace-write approval required".to_string(),
                },
                AgentProgress::ToolCallFinished {
                    name: "run_command".to_string(),
                    is_error: true,
                },
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_command_requires_workspace_write_approval_and_runs_after_approval() {
        let workspace = make_temp_dir("workspace");

        let rejected = run_tool_call(
            workspace.path(),
            "tool_cmd_touch_denied",
            "run_command",
            &json!({
                "command": ["touch", "created.txt"]
            }),
        );

        match rejected {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("requires workspace-write approval"));
            }
            _ => panic!("expected tool_result"),
        }
        assert!(!workspace.path().join("created.txt").exists());

        let approved = run_tool_call(
            workspace.path(),
            "tool_cmd_touch_ok",
            "run_command",
            &json!({
                "command": ["touch", "created.txt"],
                "approval": "workspace-write"
            }),
        );

        match approved {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, None);
                assert!(content.contains("Permission: workspace-write"));
                assert!(content.contains("Exit code: 0"));
            }
            _ => panic!("expected tool_result"),
        }
        assert!(workspace.path().join("created.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn run_command_rejects_destructive_command_without_destructive_approval() {
        let workspace = make_temp_dir("workspace");
        let target = workspace.path().join("victim.txt");
        fs::write(&target, "keep").unwrap();

        let result = run_tool_call(
            workspace.path(),
            "tool_cmd_rm",
            "run_command",
            &json!({
                "command": ["rm", "victim.txt"],
                "approval": "workspace-write"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("requires destructive approval"));
            }
            _ => panic!("expected tool_result"),
        }
        assert!(target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn run_command_captures_stderr_and_non_zero_exit_code() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_cmd_sh_exit",
            "run_command",
            &json!({
                "command": ["sh", "-c", "printf 'out'; printf 'err' >&2; exit 7"],
                "approval": "workspace-write"
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("Exit code: 7"));
                assert!(content.contains("Stdout:\nout"));
                assert!(content.contains("Stderr:\nerr"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_command_times_out() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_cmd_timeout",
            "run_command",
            &json!({
                "command": ["sh", "-c", "sleep 1"],
                "approval": "workspace-write",
                "timeout_ms": 10
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("Timed out: true"));
                assert!(content.contains("Timeout: 10 ms"));
            }
            _ => panic!("expected tool_result"),
        }
    }

    #[test]
    fn run_command_rejects_timeout_above_limit() {
        let workspace = make_temp_dir("workspace");

        let result = run_tool_call(
            workspace.path(),
            "tool_cmd_timeout_limit",
            "run_command",
            &json!({
                "command": ["pwd"],
                "timeout_ms": MAX_COMMAND_TIMEOUT_MS + 1
            }),
        );

        match result {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert_eq!(is_error, Some(true));
                assert!(content.contains("timeout_ms must be between"));
            }
            _ => panic!("expected tool_result"),
        }
    }
}
