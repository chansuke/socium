//! Whether a tool call runs, asks the user, or is refused (ADR 12).
//!
//! The model's arguments never grant anything. Commands ask by default
//! (`approval.commands = "ask"`); an allow rule in `config.toml`, a grant the
//! user gave for this session, or `"auto"` lets a sandboxed command run
//! unasked. A command that runs unasked gets a narrower sandbox: files that
//! may hold secrets are hidden and files host tools run on their own are
//! read-only. Commits always ask. Edits to those host-run files and reads of
//! likely secrets ask in every mode.

use crate::settings::{CommandApproval, PathPatterns, Settings};
use crate::tools::error::ToolError;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Most paths a command's sandbox will hide or freeze; more falls back to
/// asking, since mounting thousands of paths is slow and fragile.
const MAX_RESTRICTED_PATHS: usize = 256;

/// Something the user allowed for the rest of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    /// Commands whose arguments start with these.
    Command(Vec<String>),
    /// Edits to these workspace paths.
    Edit(Vec<String>),
    /// Reads of these workspace paths.
    Read(Vec<String>),
}

impl Grant {
    /// The choice as the approval picker offers it.
    pub fn label(&self) -> String {
        match self {
            Grant::Command(prefix) => {
                format!("Allow `{}` commands until Socium exits", prefix.join(" "))
            }
            Grant::Edit(paths) => format!("Allow edits to {} until Socium exits", paths.join(", ")),
            Grant::Read(paths) => format!("Allow reading {} until Socium exits", paths.join(", ")),
        }
    }

    fn covers(&self, other: &Grant) -> bool {
        match (self, other) {
            (Grant::Command(prefix), Grant::Command(argv)) => argv.starts_with(prefix),
            (Grant::Edit(granted), Grant::Edit(paths))
            | (Grant::Read(granted), Grant::Read(paths)) => {
                paths.iter().all(|path| granted.contains(path))
            }
            _ => false,
        }
    }
}

/// What the sandbox withholds from a command nobody reviewed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Restrictions {
    pub(crate) hidden: Vec<PathBuf>,
    pub(crate) frozen: Vec<PathBuf>,
}

pub(crate) enum Gate {
    /// Runs without asking. `None` when the call needs no restriction, the
    /// restrictions for a command nobody reviewed otherwise.
    Run(Option<Restrictions>),
    Ask(Ask),
    Deny(String),
}

pub(crate) struct Ask {
    /// One sentence on why this call asks.
    pub(crate) reason: String,
    /// Extra text for the reviewer, such as the commit diff.
    pub(crate) review: Option<String>,
    /// What "allow for this session" would grant, when it is offered.
    pub(crate) grant: Option<Grant>,
}

/// Decides how `name` with `input` may run.
pub(crate) fn gate(
    name: &str,
    input: &Value,
    root: &Path,
    settings: &Settings,
    granted: &dyn Fn(&Grant) -> bool,
) -> Result<Gate, ToolError> {
    let approval = &settings.approval;
    match name {
        "git_commit" => Ok(Gate::Ask(Ask {
            reason: "A commit changes the repository history.".into(),
            review: Some(crate::tools::git::commit_review(root, input)?),
            grant: None,
        })),
        "run_command" => {
            let Some(argv) = command_argv(input) else {
                // Malformed input fails in the tool before anything runs.
                return Ok(Gate::Run(None));
            };
            if let Some(rule) = approval.deny.iter().find(|rule| argv.starts_with(rule)) {
                return Ok(Gate::Deny(format!(
                    "`{}` is denied by approval.deny in Socium's config.toml; do not retry it",
                    rule.join(" ")
                )));
            }
            let suggested = suggest_prefix(&argv);
            let allowed = approval.allow.iter().any(|rule| argv.starts_with(rule))
                || granted(&Grant::Command(argv.clone()))
                || approval.commands == CommandApproval::Auto;
            if allowed {
                match restrictions(root, approval) {
                    Ok(restrictions) => return Ok(Gate::Run(Some(restrictions))),
                    Err(reason) => {
                        return Ok(Gate::Ask(Ask {
                            reason: format!(
                                "This command would run without asking, but {reason}, so it needs a review."
                            ),
                            review: None,
                            grant: None,
                        }));
                    }
                }
            }
            Ok(Gate::Ask(Ask {
                reason: "Commands run in the sandbox but can change or delete workspace files."
                    .into(),
                review: None,
                grant: suggested.map(Grant::Command),
            }))
        }
        _ => {
            let edits = edited_paths(name, input, root);
            let protected = edits
                .iter()
                .filter(|path| approval.protected.matches(path))
                .cloned()
                .collect::<Vec<_>>();
            if !protected.is_empty() && !granted(&Grant::Edit(protected.clone())) {
                return Ok(Gate::Ask(Ask {
                    reason: format!(
                        "{} {} run or loaded by host tools on their own (direnv, rust-analyzer, editors, CI), outside the sandbox.",
                        protected.join(", "),
                        if protected.len() == 1 { "is" } else { "are" }
                    ),
                    review: None,
                    grant: Some(Grant::Edit(protected)),
                }));
            }
            if let Some(path) = read_path(name, input, root)
                && approval.sensitive.matches(&path)
                && !granted(&Grant::Read(vec![path.clone()]))
            {
                return Ok(Gate::Ask(Ask {
                    reason: format!(
                        "{path} may hold credentials, and what is read is sent to the model provider."
                    ),
                    review: None,
                    grant: Some(Grant::Read(vec![path])),
                }));
            }
            Ok(Gate::Run(None))
        }
    }
}

/// Whether a session grant covers `wanted`.
pub(crate) fn grants_cover(grants: &[Grant], wanted: &Grant) -> bool {
    grants.iter().any(|grant| grant.covers(wanted))
}

fn command_argv(input: &Value) -> Option<Vec<String>> {
    input["command"]
        .as_array()?
        .iter()
        .map(|part| part.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()
        .filter(|argv| !argv.is_empty())
}

/// The prefix "allow for this session" offers: the program and the
/// subcommand words after it, such as `cargo test` or `npm run build`.
/// Shells, interpreters running inline code, and wrappers that run another
/// program get no offer, since their first words say nothing about what
/// runs.
pub(crate) fn suggest_prefix(argv: &[String]) -> Option<Vec<String>> {
    let program = argv.first()?;
    let name = Path::new(program).file_name()?.to_str()?;
    const WRAPPERS: &[&str] = &[
        "sh",
        "bash",
        "zsh",
        "fish",
        "dash",
        "env",
        "xargs",
        "sudo",
        "doas",
        "timeout",
        "nice",
        "nohup",
        "time",
        "eval",
        "exec",
        "command",
        "nix",
        "nix-shell",
        "busybox",
        "find",
        "awk",
        "perl",
        "ruby",
    ];
    if WRAPPERS.contains(&name) {
        return None;
    }
    let mut prefix = vec![program.clone()];
    let interpreter = matches!(name, "python" | "python3" | "node" | "deno" | "bun");
    for argument in argv.iter().skip(1).take(2) {
        if interpreter && argument == "-m" {
            prefix.push(argument.clone());
            continue;
        }
        let word = !argument.is_empty()
            && argument
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
            && !argument.starts_with('-');
        if !word {
            break;
        }
        prefix.push(argument.clone());
    }
    if interpreter && prefix.len() == 1 {
        // `python script.py` or `node -e ...`: the script decides.
        return None;
    }
    Some(prefix)
}

/// Workspace paths (from the root, `/`-separated) a file tool would change.
fn edited_paths(name: &str, input: &Value, root: &Path) -> Vec<String> {
    let requested = match name {
        "apply_patch" => input["patch"]
            .as_str()
            .and_then(|patch| crate::tools::apply_patch::patch_paths(patch).ok())
            .unwrap_or_default(),
        crate::constants::TEXT_EDITOR_TOOL_NAME
            if matches!(
                input["command"].as_str(),
                Some("create" | "str_replace" | "insert" | "undo_edit")
            ) =>
        {
            input["path"]
                .as_str()
                .map(str::to_string)
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    };
    requested
        .iter()
        .filter_map(|path| relative_target(root, path))
        .collect()
}

/// The workspace path a read tool would read.
fn read_path(name: &str, input: &Value, root: &Path) -> Option<String> {
    let path = match name {
        "read_file" => input["path"].as_str()?,
        crate::constants::TEXT_EDITOR_TOOL_NAME if input["command"] == "view" => {
            input["path"].as_str()?
        }
        _ => return None,
    };
    relative_target(root, path)
}

/// `requested` as it resolves inside the workspace, symlinks followed, so
/// an alias cannot dodge a pattern. `None` when it does not resolve; the
/// tool then reports the error.
pub(crate) fn relative_target(root: &Path, requested: &str) -> Option<String> {
    let resolved =
        crate::tools::workspace::resolve_workspace_path_for_create(root, requested).ok()?;
    let root = root.canonicalize().ok()?;
    let relative = resolved.strip_prefix(&root).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

/// The files a command nobody reviewed may not read (`sensitive`) or change
/// (`protected`), found with ripgrep, ignored files included. A directory
/// that matches as a whole is frozen as a whole, so new files cannot be
/// added to it either.
fn restrictions(
    root: &Path,
    approval: &crate::settings::ApprovalSettings,
) -> Result<Restrictions, String> {
    let candidates = find_matching_files(
        root,
        approval
            .protected
            .patterns()
            .iter()
            .chain(approval.sensitive.patterns()),
    )?;
    let mut restrictions = Restrictions::default();
    let mut frozen_dirs: Vec<String> = Vec::new();
    for file in candidates {
        if approval.sensitive.matches(&file) {
            restrictions.hidden.push(root.join(&file));
        }
        if approval.protected.matches(&file) {
            let target = shallowest_matching_dir(&file, &approval.protected).unwrap_or(file);
            if !frozen_dirs
                .iter()
                .any(|dir| target.starts_with(&format!("{dir}/")) || *dir == target)
            {
                frozen_dirs.push(target);
            }
        }
    }
    // Directory patterns whose directory exists at the root but holds no
    // file yet.
    for pattern in approval.protected.patterns() {
        if let Some(dir) = pattern.strip_suffix("/**")
            && !pattern.starts_with('!')
        {
            let dir = dir.strip_prefix("**/").unwrap_or(dir);
            if !dir.contains('*')
                && root.join(dir).is_dir()
                && !frozen_dirs.iter().any(|known| known == dir)
            {
                frozen_dirs.push(dir.to_string());
            }
        }
    }
    let all = frozen_dirs.clone();
    frozen_dirs.retain(|dir| {
        !all.iter()
            .any(|other| other != dir && dir.starts_with(&format!("{other}/")))
    });
    restrictions.frozen = frozen_dirs.iter().map(|dir| root.join(dir)).collect();
    // A hidden file under a frozen directory is mounted after it, so it
    // stays hidden.
    if restrictions.hidden.len() + restrictions.frozen.len() > MAX_RESTRICTED_PATHS {
        return Err(format!(
            "the workspace has more than {MAX_RESTRICTED_PATHS} protected or sensitive files to withhold"
        ));
    }
    Ok(restrictions)
}

fn shallowest_matching_dir(file: &str, patterns: &PathPatterns) -> Option<String> {
    let parts = file.split('/').collect::<Vec<_>>();
    (1..parts.len())
        .map(|end| parts[..end].join("/"))
        .find(|dir| patterns.matches(dir))
}

/// Workspace files matching any of the positive `patterns`, ignored files
/// and hidden files included, `.git` and `.socium` excluded.
fn find_matching_files<'a>(
    root: &Path,
    patterns: impl Iterator<Item = &'a String>,
) -> Result<Vec<String>, String> {
    let mut command = vec![
        "rg".to_string(),
        "--no-config".into(),
        "--files".into(),
        "--hidden".into(),
        "--no-ignore".into(),
        "--null".into(),
    ];
    let mut any = false;
    for pattern in patterns.filter(|pattern| !pattern.starts_with('!')) {
        command.push("--glob".into());
        command.push(pattern.clone());
        any = true;
    }
    if !any {
        return Ok(Vec::new());
    }
    command.extend([
        "--glob".into(),
        "!.git".into(),
        "--glob".into(),
        "!.socium".into(),
    ]);
    let output = crate::process::run(
        &command,
        root,
        Duration::from_secs(5),
        &crate::TaskControl::default(),
    )
    .map_err(|err| format!("protected files could not be listed ({err})"))?;
    if output.timed_out || output.truncated || !matches!(output.status.code(), Some(0 | 1)) {
        return Err("protected files could not be listed in time".into());
    }
    let mut files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            String::from_utf8_lossy(path)
                .trim_start_matches("./")
                .to_string()
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn session_offers_name_the_program_and_its_subcommand() {
        for (argv, expected) in [
            ("cargo test --workspace", Some("cargo test")),
            ("npm run build", Some("npm run build")),
            ("git status", Some("git status")),
            ("pytest -x tests", Some("pytest")),
            ("python -m pytest -q", Some("python -m pytest")),
            ("./scripts/check.sh fast", Some("./scripts/check.sh fast")),
            ("sh -c rm", None),
            ("env FOO=1 make", None),
            ("python script.py", None),
            ("node -e code", None),
        ] {
            assert_eq!(
                suggest_prefix(&words(argv)).map(|prefix| prefix.join(" ")),
                expected.map(str::to_string),
                "{argv}"
            );
        }
    }

    #[test]
    fn grants_cover_longer_commands_and_listed_paths_only() {
        let grants = [
            Grant::Command(words("cargo test")),
            Grant::Edit(vec!["build.rs".into()]),
        ];
        assert!(grants_cover(
            &grants,
            &Grant::Command(words("cargo test -p core"))
        ));
        assert!(!grants_cover(
            &grants,
            &Grant::Command(words("cargo build"))
        ));
        assert!(grants_cover(&grants, &Grant::Edit(vec!["build.rs".into()])));
        assert!(!grants_cover(
            &grants,
            &Grant::Edit(vec!["build.rs".into(), ".envrc".into()])
        ));
        assert!(!grants_cover(
            &grants,
            &Grant::Read(vec!["build.rs".into()])
        ));
    }

    fn decide(root: &Path, settings: &Settings, name: &str, input: Value) -> Gate {
        gate(name, &input, root, settings, &|_| false).unwrap()
    }

    #[test]
    fn commands_ask_by_default_and_rules_decide_otherwise() {
        let root = tempfile::tempdir().unwrap();
        let (settings, _) = Settings::from_toml(
            "[approval]\nallow = [\"cargo test\"]\ndeny = [\"git push\"]",
            None,
        )
        .unwrap();
        let run = |argv: &str| {
            decide(
                root.path(),
                &settings,
                "run_command",
                json!({"command": words(argv)}),
            )
        };
        assert!(matches!(
            run("cargo build"),
            Gate::Ask(Ask { grant: Some(_), .. })
        ));
        assert!(matches!(run("cargo test -p x"), Gate::Run(Some(_))));
        assert!(matches!(run("git push origin"), Gate::Deny(_)));
        let (auto, _) =
            Settings::from_toml("[approval]\ncommands = \"auto\"\ndeny = [\"rm\"]", None).unwrap();
        assert!(matches!(
            decide(
                root.path(),
                &auto,
                "run_command",
                json!({"command": ["make"]})
            ),
            Gate::Run(Some(_))
        ));
        assert!(matches!(
            decide(
                root.path(),
                &auto,
                "run_command",
                json!({"command": ["rm", "-rf", "."]})
            ),
            Gate::Deny(_)
        ));
        // A commit asks even in auto mode.
        crate::test_support::init_git_repo(root.path());
        std::fs::write(root.path().join("tracked.txt"), "changed\n").unwrap();
        assert!(matches!(
            decide(
                root.path(),
                &auto,
                "git_commit",
                json!({"message": "m", "paths": ["tracked.txt"]})
            ),
            Gate::Ask(Ask { grant: None, .. })
        ));
    }

    #[test]
    fn protected_edits_and_sensitive_reads_ask_in_every_mode() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".env"), "TOKEN=fake\n").unwrap();
        std::fs::write(root.path().join(".env.example"), "TOKEN=\n").unwrap();
        let (auto, _) = Settings::from_toml("[approval]\ncommands = \"auto\"", None).unwrap();
        let edit = |path: &str| {
            decide(
                root.path(),
                &auto,
                crate::constants::TEXT_EDITOR_TOOL_NAME,
                json!({"command": "create", "path": path, "file_text": "x"}),
            )
        };
        assert!(matches!(edit("build.rs"), Gate::Ask(_)));
        assert!(matches!(edit(".github/workflows/ci.yaml"), Gate::Ask(_)));
        assert!(matches!(edit("src/lib.rs"), Gate::Run(None)));
        let patch =
            "*** Begin Patch\n*** Add File: crates/x/build.rs\n+fn main() {}\n*** End Patch";
        assert!(matches!(
            decide(root.path(), &auto, "apply_patch", json!({"patch": patch})),
            Gate::Ask(Ask {
                grant: Some(Grant::Edit(_)),
                ..
            })
        ));
        assert!(matches!(
            decide(root.path(), &auto, "read_file", json!({"path": ".env"})),
            Gate::Ask(_)
        ));
        assert!(matches!(
            decide(
                root.path(),
                &auto,
                "read_file",
                json!({"path": ".env.example"})
            ),
            Gate::Run(None)
        ));
        let granted = |grant: &Grant| *grant == Grant::Read(vec![".env".into()]);
        assert!(matches!(
            gate(
                "read_file",
                &json!({"path": "./.env"}),
                root.path(),
                &auto,
                &granted
            )
            .unwrap(),
            Gate::Run(None)
        ));
    }

    #[test]
    fn unreviewed_commands_lose_secrets_and_host_run_files() {
        let root = tempfile::tempdir().unwrap();
        for file in [
            ".env",
            "app/.env.local",
            ".github/workflows/ci.yaml",
            "build.rs",
            "src/lib.rs",
        ] {
            let path = root.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        std::fs::create_dir_all(root.path().join(".vscode")).unwrap();
        let restrictions = restrictions(root.path(), &Settings::default().approval).unwrap();
        let names = |paths: &[PathBuf]| {
            paths
                .iter()
                .map(|path| {
                    path.strip_prefix(root.path())
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&restrictions.hidden), [".env", "app/.env.local"]);
        let mut frozen = names(&restrictions.frozen);
        frozen.sort();
        assert_eq!(frozen, [".github/workflows", ".vscode", "build.rs"]);
    }
}
