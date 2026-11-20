use crate::AgentProgress;
use crate::constants::*;
use crate::execution::{ExecutionPolicy, WorkspaceAccess};
use crate::tools::error::ToolError;
use crate::tools::input::{optional_string_field, optional_timeout_ms, require_string_array_field};
use crate::tools::workspace::resolve_workspace_command_cwd;
use crate::tools::{Tool, ToolContext, ToolDefinition};
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
    restrictions: Option<&crate::approval::Restrictions>,
    on_progress: &mut F,
) -> Result<String, ToolError>
where
    F: FnMut(AgentProgress),
{
    let RunCommandInput {
        command,
        cwd,
        timeout_ms,
    } = RunCommandInput::parse(input, sandbox.max_command_timeout)?;
    let resolved_cwd = resolve_workspace_command_cwd(workspace_root, cwd)?;
    on_progress(AgentProgress::CommandStarted {
        command: display_command(&command),
        cwd: cwd.to_string(),
    });
    let mut policy = ExecutionPolicy::new(workspace_root, WorkspaceAccess::Write)
        .map_err(|e| e.to_string())?
        .with_settings(sandbox);
    if let Some(restrictions) = restrictions {
        policy.hidden = restrictions.hidden.clone();
        policy.frozen = restrictions.frozen.clone();
    }
    let execution = execute_command(
        &command,
        &resolved_cwd,
        cwd,
        timeout_ms,
        control,
        workspace_root,
        &policy,
        on_progress,
    );
    // What the command wrote is checked however it ended.
    let withheld = match restrictions {
        Some(restrictions) => restrictions.withhold_changes(workspace_root)?,
        None => None,
    };
    let execution = match execution {
        Ok(execution) => execution,
        Err(err) => return Err(format!("{err}{}", withheld.unwrap_or_default()).into()),
    };
    let cut = |stream: &str| stream.chars().count() > MAX_COMMAND_OUTPUT_CHARACTERS;
    on_progress(AgentProgress::CommandFinished {
        command: execution.command_display.clone(),
        exit_code: execution.exit_code,
        timed_out: execution.timed_out,
        truncated: cut(&execution.stdout) || cut(&execution.stderr),
        stdout_preview: format_command_stream(&execution.stdout)
            .trim_end_matches('\n')
            .to_string(),
        stderr_preview: format_command_stream(&execution.stderr)
            .trim_end_matches('\n')
            .to_string(),
    });

    let mut output = format_command_execution(&execution);
    if let Some(note) = &withheld {
        output.push_str(note);
    }
    if withheld.is_some()
        || control.is_cancelled()
        || execution.timed_out
        || execution.exit_code != Some(0)
    {
        Err(output.into())
    } else {
        Ok(output)
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_command(
    command: &[String],
    cwd: &Path,
    cwd_display: &str,
    timeout_ms: u64,
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
    prune_outputs(workspace_root);
    let mut files = [open("stdout")?, open("stderr")?];
    let mut totals = [0u64; 2];
    let mut reported = std::time::Instant::now();
    const ARTIFACT_LIMIT: u64 = crate::constants::MAX_COMMAND_ARTIFACT_BYTES;
    let mut observer = |stderr: bool, bytes: &[u8]| -> std::io::Result<()> {
        #[cfg(test)]
        if tests::FAIL_OUTPUT.get() {
            return Err(std::io::Error::other("no space left on device"));
        }
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
    let output=crate::process::run_observed(command,cwd,Duration::from_millis(timeout_ms),control,&mut observer, policy)
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

/// Deletes the saved output of all but the newest
/// `MAX_KEPT_COMMAND_OUTPUTS - 1` commands, oldest first, so the next one
/// fits. Best effort: a file that cannot be removed is left.
fn prune_outputs(workspace_root: &Path) {
    // The checked path of a file in it, as `open` below gets: the state
    // path checks accept files, not directories.
    let Some(dir) = crate::agent::state::workspace_state_path(workspace_root, "outputs/0")
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
    use crate::{TaskControl, agent::types::ContentBlock, tools::run_tool_call_controlled};

    /// Freezing the protected files that exist does not stop a command
    /// nobody reviewed from creating new ones, or from moving a frozen
    /// directory's parent aside and writing a fresh one in its place.
    #[test]
    fn an_unreviewed_command_cannot_leave_files_host_tools_run() {
        let settings =
            crate::settings::Settings::from_toml("[approval]\ncommands = \"auto\"\n", None)
                .unwrap()
                .0;
        for (script, created) in [
            ("echo x > .envrc", ".envrc"),
            ("ln -s notes.txt .envrc", ".envrc"),
            (
                "mv .github .gh2 && mkdir -p .github/workflows && echo evil > .github/workflows/x.yml",
                ".github/workflows/x.yml",
            ),
            (
                "mkdir -p crates/a && echo 'fn main() {}' > crates/a/build.rs",
                "crates/a/build.rs",
            ),
            (
                "mkdir -p crates/b && echo 'fn main() {}' > crates/b/build.rs && chmod 111 crates/b",
                "crates/b/build.rs",
            ),
            (
                "mkdir -p crates/c && echo 'fn main() {}' > crates/c/build.rs && chmod 444 crates/c",
                "crates/c/build.rs",
            ),
            (
                "mkdir -p crates/d && echo 'fn main() {}' > crates/d/build.rs && chmod 555 crates/d",
                "crates/d/build.rs",
            ),
            (
                "mkdir -p crates/e/sub && echo 'fn main() {}' > crates/e/sub/build.rs && chmod 400 crates/e",
                "crates/e/sub/build.rs",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
            std::fs::write(root.path().join(".github/workflows/ci.yaml"), "on: push\n").unwrap();
            let input = json!({"command": ["sh", "-c", script]});
            let crate::approval::Gate::Run(Some(restrictions)) =
                crate::approval::gate("run_command", &input, root.path(), &settings, &|_| false)
                    .unwrap()
            else {
                panic!("an auto-approved command runs restricted");
            };
            let result = run_command_controlled(
                root.path(),
                &input,
                &TaskControl::default(),
                &settings.sandbox,
                Some(&restrictions),
                &mut |_| {},
            );
            let message = result.expect_err(script).to_string();
            assert!(message.contains(created), "{script}: {message}");
            assert!(
                root.path().join(created).symlink_metadata().is_err(),
                "{script} left {created}"
            );
            let quarantine = root.path().join(".socium/quarantine");
            let kept = std::fs::read_dir(&quarantine)
                .unwrap()
                .flatten()
                .any(|stamp| stamp.path().join(created).symlink_metadata().is_ok());
            assert!(kept, "{script}: {created} is not in quarantine");
        }
        let root = tempfile::tempdir().unwrap();
        let input = json!({"command": ["sh", "-c", "echo x > notes.txt"]});
        let crate::approval::Gate::Run(Some(restrictions)) =
            crate::approval::gate("run_command", &input, root.path(), &settings, &|_| false)
                .unwrap()
        else {
            panic!("an auto-approved command runs restricted");
        };
        run_command_controlled(
            root.path(),
            &input,
            &TaskControl::default(),
            &settings.sandbox,
            Some(&restrictions),
            &mut |_| {},
        )
        .unwrap();
        assert!(root.path().join("notes.txt").exists());
    }

    thread_local! {
        /// Makes saving a command's output fail, as on a full disk.
        pub(super) static FAIL_OUTPUT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn run_unreviewed(root: &Path, script: &str) -> Result<String, ToolError> {
        let settings =
            crate::settings::Settings::from_toml("[approval]\ncommands = \"auto\"\n", None)
                .unwrap()
                .0;
        let input = json!({"command": ["sh", "-c", script]});
        let crate::approval::Gate::Run(Some(restrictions)) =
            crate::approval::gate("run_command", &input, root, &settings, &|_| false).unwrap()
        else {
            panic!("an auto-approved command runs restricted");
        };
        run_command_controlled(
            root,
            &input,
            &TaskControl::default(),
            &settings.sandbox,
            Some(&restrictions),
            &mut |_| {},
        )
    }

    #[test]
    fn a_command_whose_output_cannot_be_saved_still_loses_new_protected_files() {
        let root = tempfile::tempdir().unwrap();
        FAIL_OUTPUT.set(true);
        let result = run_unreviewed(root.path(), "echo x > .envrc; echo done");
        FAIL_OUTPUT.set(false);
        let message = result.expect_err("the output was not saved").to_string();
        assert!(message.contains("output capture failed"), "{message}");
        assert!(message.contains(".envrc"), "{message}");
        assert!(root.path().join(".envrc").symlink_metadata().is_err());
    }

    /// Cargo takes a crate linked into `crates/` as a member, so a new
    /// link to one is a new build script, though no file behind it changed.
    #[test]
    fn a_command_cannot_link_a_crate_into_the_workspace() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("build.rs"), "fn main() {}\n").unwrap();
        let outside = outside.path().display();
        for (script, links) in [
            ("ln -s ../fixtures/evil crates/evil", vec!["crates/evil"]),
            (
                &format!("ln -s {outside} crates/outside"),
                vec!["crates/outside"],
            ),
            (
                "ln -s ../fixtures/evil crates/a && ln -s ../fixtures/evil crates/b",
                vec!["crates/a", "crates/b"],
            ),
            ("ln -s ../fixtures crates/nest", vec!["crates/nest"]),
        ] {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(root.path().join("fixtures/evil")).unwrap();
            std::fs::create_dir_all(root.path().join("crates")).unwrap();
            std::fs::write(root.path().join("fixtures/evil/build.rs"), "fn main() {}\n").unwrap();
            let message = run_unreviewed(root.path(), script)
                .expect_err(script)
                .to_string();
            for link in links {
                assert!(message.contains(link), "{script}: {message}");
                assert!(
                    root.path().join(link).symlink_metadata().is_err(),
                    "{script} left {link}"
                );
            }
            assert!(root.path().join("fixtures/evil/build.rs").exists());
        }
    }

    /// Commands may change `.socium/scratch`, which the listing skips, so a
    /// link into it is followed every time.
    #[test]
    fn a_link_into_scratch_cannot_bring_a_build_script_later() {
        let root = tempfile::tempdir().unwrap();
        run_unreviewed(
            root.path(),
            "mkdir .socium/scratch/s && ln -s .socium/scratch/s s",
        )
        .unwrap();
        let message = run_unreviewed(
            root.path(),
            "echo 'fn main() {}' > .socium/scratch/s/build.rs",
        )
        .expect_err("a build script behind a link")
        .to_string();
        assert!(message.contains("s"), "{message}");
        assert!(root.path().join("s").symlink_metadata().is_err());
    }

    #[test]
    fn a_protected_file_the_quarantine_cannot_take_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let quarantine =
            crate::agent::state::workspace_state_path(root.path(), "quarantine").unwrap();
        std::fs::write(quarantine, "").unwrap();
        let message = run_unreviewed(root.path(), "echo x > .envrc")
            .expect_err("a new .envrc is an error")
            .to_string();
        assert!(message.contains(".envrc"), "{message}");
        assert!(root.path().join(".envrc").symlink_metadata().is_err());
    }

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
        let outputs = crate::agent::state::workspace_state_path(workspace.path(), "outputs/x")
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
