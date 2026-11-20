use crate::AgentProgress;
use crate::constants::*;
use crate::conversation::ToolDefinition;
use crate::execution::{ExecutionPolicy, WorkspaceAccess};
use crate::tool_error::ToolError;
use crate::tools::input::{optional_string_field, optional_timeout_ms, require_string_array_field};
use crate::tools::{Tool, ToolContext};
use crate::workspace::resolve_workspace_command_cwd;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) struct RunCommandTool;

impl Tool for RunCommandTool {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn definition(&self) -> ToolDefinition {
        definition(crate::settings::DEFAULT_MAX_COMMAND_TIMEOUT)
    }

    fn run(
        &self,
        ctx: &ToolContext<'_>,
        input: &Value,
        on_progress: &mut dyn FnMut(AgentProgress),
    ) -> Result<String, ToolError> {
        let mut forward = |progress| on_progress(progress);
        run_command_controlled(
            ctx.workspace_root,
            input,
            ctx.control,
            &ctx.settings.sandbox,
            ctx.restrictions,
            &mut forward,
        )
    }
}

/// The tool definition with the user's longest allowed timeout.
pub(crate) fn definition(max_timeout: Duration) -> ToolDefinition {
    let max_ms = max_timeout.as_millis();
    ToolDefinition {
            name: "run_command".to_string(),
            description: "Run a command in the Linux OS sandbox. Pass the executable and its arguments as an array; nothing is interpreted by a shell unless you run one. Workspace files are writable; Git metadata is read-only and agent state is hidden. System runtimes and directories the user exposed are read-only. There is no network, and the host HOME and credentials are unavailable. Depending on the user's settings the command runs at once or after the user approves it; a denied command must not be retried. Output beyond the limit is cut from the middle and saved in full under .socium/outputs; read the saved files with read_file and offset. Use git_commit for commits."
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
                        "description": format!("Optional timeout in milliseconds, default {DEFAULT_COMMAND_TIMEOUT_MS}. Must be between 1 and {max_ms}.")
                    }

                },
                "required": ["command"],
                "additionalProperties": false
            }),
        }
}

struct CommandExecution {
    artifacts: String,
    /// The saved streams' path without its `.stdout` or `.stderr`.
    saved_output: String,
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
    fn parse(input: &'a Value, max_timeout: Duration) -> Result<Self, ToolError> {
        let input_object = input
            .as_object()
            .ok_or("run_command requires an object input")?;
        let command = require_string_array_field(input_object, "command", "run_command")?;
        let cwd = optional_string_field(input_object, "cwd", "run_command cwd")?.unwrap_or(".");
        let timeout_ms = optional_timeout_ms(input_object, max_timeout.as_millis() as u64)?;
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
        &crate::settings::SandboxSettings::default(),
        None,
        on_progress,
    )
}

fn run_command_controlled<F>(
    workspace_root: &Path,
    input: &Value,
    control: &crate::TaskControl,
    sandbox: &crate::settings::SandboxSettings,
    restrictions: Option<&crate::access::Restrictions>,
    on_progress: &mut F,
) -> Result<String, ToolError>
where
    F: FnMut(AgentProgress),
{
    let request = RunCommandInput::parse(input, sandbox.max_command_timeout)?;
    let resolved_cwd = resolve_workspace_command_cwd(workspace_root, request.cwd)?;
    on_progress(AgentProgress::CommandStarted {
        command: display_command(&request.command),
        cwd: request.cwd.to_string(),
    });
    let mut policy = ExecutionPolicy::new(workspace_root, WorkspaceAccess::Write)
        .map_err(|e| e.to_string())?
        .with_settings(sandbox);
    if let Some(restrictions) = restrictions {
        policy.hidden = restrictions.hidden.clone();
        policy.frozen = restrictions.frozen.clone();
    }
    let execution = execute_command(
        &request,
        &resolved_cwd,
        control,
        workspace_root,
        &policy,
        on_progress,
    )?;
    let cut = |stream: &str| stream.chars().count() > MAX_COMMAND_OUTPUT_CHARACTERS;
    on_progress(AgentProgress::CommandFinished {
        command: execution.command_display.clone(),
        exit_code: execution.exit_code,
        timed_out: execution.timed_out,
        truncated: cut(&execution.stdout) || cut(&execution.stderr),
        saved_output: execution.saved_output.clone(),
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
    request: &RunCommandInput,
    cwd: &Path,
    control: &crate::TaskControl,
    workspace_root: &Path,
    policy: &ExecutionPolicy,
    on_progress: &mut dyn FnMut(AgentProgress),
) -> Result<CommandExecution, String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let name = format!(
        "outputs/{}-{timestamp}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let stem = format!(".socium/{name}");
    let open = |suffix: &str| -> Result<std::fs::File, String> {
        let path = crate::state::workspace_state_path(workspace_root, &format!("{name}.{suffix}"))
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
    prune_outputs(workspace_root);
    let mut files = [open("stdout")?, open("stderr")?];
    let mut totals = [0u64; 2];
    let mut reported = std::time::Instant::now();
    const ARTIFACT_LIMIT: u64 = crate::constants::MAX_COMMAND_ARTIFACT_BYTES;
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
    let output=crate::process::run_observed(&request.command,cwd,Duration::from_millis(request.timeout_ms),control,&mut observer, policy)
        .map_err(|e|format!("Command failed or output capture failed: {e}; partial artifacts: {stem}.stdout / {stem}.stderr"))?;
    for file in &files {
        file.sync_all()
            .map_err(|e| format!("Command completed but output sync failed: {e}"))?;
    }
    let artifacts = format!(
        "{stem}.stdout ({} bytes), {stem}.stderr ({} bytes); artifact cap 8 MiB per stream, truncated={}; read omitted output with read_file and offset",
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

    if output.cancelled {
        stdout.push_str("\n(command cancelled)");
    }

    Ok(CommandExecution {
        artifacts,
        saved_output: stem,
        stop_reason,
        command_display: display_command(&request.command),
        cwd_display: request.cwd.to_string(),
        timeout_ms: request.timeout_ms,
        timed_out,
        exit_code: output.status.code(),
        stdout,
        stderr,
    })
}

/// Deletes the saved output of all but the newest
/// `MAX_KEPT_COMMAND_OUTPUTS - 1` commands, oldest first, so the next one
/// fits. Best effort: a file that cannot be removed is left.
fn prune_outputs(workspace_root: &Path) {
    // The checked path of a file in it, as `open` below gets: the state
    // path checks accept files, not directories.
    let Some(dir) = crate::state::workspace_state_path(workspace_root, "outputs/0")
        .ok()
        .and_then(|file| file.parent().map(Path::to_path_buf))
    else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    // `<pid>-<nanos>-<n>.stdout|stderr`: one command per stem.
    let mut commands =
        std::collections::BTreeMap::<String, (std::time::SystemTime, Vec<PathBuf>)>::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        let command = commands
            .entry(stem.to_string())
            .or_insert((modified, Vec::new()));
        command.0 = command.0.max(modified);
        command.1.push(path);
    }
    let mut commands = commands.into_values().collect::<Vec<_>>();
    commands.sort_by_key(|(modified, _)| *modified);
    let excess = (commands.len() + 1).saturating_sub(crate::constants::MAX_KEPT_COMMAND_OUTPUTS);
    for (_, paths) in commands.into_iter().take(excess) {
        for path in paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn format_command_execution(execution: &CommandExecution) -> String {
    let stdout = format_command_stream(&execution.stdout);
    let stderr = format_command_stream(&execution.stderr);
    let exit_code = match execution.exit_code {
        Some(code) => code.to_string(),
        None => "terminated".to_string(),
    };

    format!(
        "Command: {}\nCwd: {}\nPermission: approved Linux sandbox; workspace-write, Git read-only, network denied\nStop reason: {}\nArtifacts: {}\nTimeout: {} ms\nTimed out: {}\nExit code: {}\nStdout:\n{}\nStderr:\n{}",
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

    // Failures and summaries are at the end, so most of the room goes to
    // the tail.
    let length = stream.chars().count();
    if length <= MAX_COMMAND_OUTPUT_CHARACTERS {
        return stream.to_string();
    }
    let head = MAX_COMMAND_OUTPUT_CHARACTERS / 10;
    let tail = MAX_COMMAND_OUTPUT_CHARACTERS - head;
    format!(
        "{}\n[... {} characters omitted; the artifact file has the whole stream ...]\n{}",
        stream.chars().take(head).collect::<String>(),
        length - head - tail,
        stream.chars().skip(length - tail).collect::<String>()
    )
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
    use crate::{TaskControl, conversation::ContentBlock, tools::run_tool_call_controlled};
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

    #[test]
    fn the_finished_event_names_the_files_that_hold_both_streams() {
        let workspace = crate::test_support::make_temp_dir("saved_output");
        let mut saved = None;
        run_command_from_workspace_with_progress(
            workspace.path(),
            &json!({"command":["sh","-c","echo out; echo err >&2; exit 3"]}),
            &mut |event| {
                if let AgentProgress::CommandFinished { saved_output, .. } = event {
                    saved = Some(saved_output);
                }
            },
        )
        .unwrap_err();
        let saved = workspace.path().join(saved.unwrap());
        let read = |suffix: &str| {
            std::fs::read_to_string(format!("{}.{suffix}", saved.display())).unwrap()
        };
        assert_eq!(read("stdout"), "out\n");
        assert_eq!(read("stderr"), "err\n");
    }

    #[test]
    fn long_output_shows_mostly_its_end() {
        let stream = format!(
            "head\n{}\nerror: the real failure\n",
            "noise\n".repeat(10_000)
        );
        let shown = super::format_command_stream(&stream);
        assert!(shown.starts_with("head\n"));
        assert!(shown.ends_with("error: the real failure\n"));
        assert!(shown.contains("characters omitted"));
        assert!(shown.chars().count() < MAX_COMMAND_OUTPUT_CHARACTERS + 100);
    }

    #[test]
    fn only_the_newest_command_outputs_are_kept() {
        let workspace = crate::test_support::make_temp_dir("prune_outputs");
        // Created the way commands create it, with private permissions.
        let outputs = crate::state::workspace_state_path(workspace.path(), "outputs/x")
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let total = MAX_KEPT_COMMAND_OUTPUTS + 20;
        for n in 0..total {
            for stream in ["stdout", "stderr"] {
                let path = outputs.join(format!("1-{n:04}-0.{stream}"));
                std::fs::write(&path, "x").unwrap();
                let at = std::time::UNIX_EPOCH + Duration::from_secs(1_000 + n as u64);
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(at)
                    .unwrap();
            }
        }

        prune_outputs(workspace.path());

        let mut left = std::fs::read_dir(&outputs)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        left.sort();
        assert_eq!(left.len(), 2 * (MAX_KEPT_COMMAND_OUTPUTS - 1));
        assert_eq!(
            left[0],
            format!("1-{:04}-0.stderr", total - MAX_KEPT_COMMAND_OUTPUTS + 1)
        );
    }
}
