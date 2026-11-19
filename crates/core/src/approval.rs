//! Whether a tool call runs, asks the user, or is refused (ADR 13).
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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Most paths a command's sandbox will hide or freeze; more falls back to
/// asking, since mounting thousands of paths is slow and fragile.
const MAX_RESTRICTED_PATHS: usize = 256;

/// How long listing the workspace's protected and sensitive files may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// What the grant covers, as the approval picker names it after
    /// "Allow".
    pub fn scope(&self) -> String {
        match self {
            Grant::Command(prefix) => format!("`{}` commands", prefix.join(" ")),
            Grant::Edit(paths) => format!("edits to {}", paths.join(", ")),
            Grant::Read(paths) => format!("reading {}", paths.join(", ")),
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
    /// Freezing covers only files that exist, so the protected files at the
    /// start are kept to find any the command created or replaced.
    protected: PathPatterns,
    before: BTreeMap<String, FileId>,
}

/// A path's device and inode, so a file replaced at the same path shows.
type FileId = (u64, u64);

impl Restrictions {
    /// Moves protected files the command created or replaced into
    /// `.socium/quarantine`, where no host tool finds them and no command
    /// reaches them, and says what it moved and what is gone. `None` when
    /// the command left them as they were.
    pub(crate) fn withhold_changes(&self, root: &Path) -> Result<Option<String>, String> {
        let after =
            find_matching_files(root, &|path| self.protected.matches(path), Listing::After)?;
        let changed = after
            .iter()
            .filter(|(path, id)| self.before.get(path) != Some(id))
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        let missing = self
            .before
            .keys()
            .filter(|path| !after.iter().any(|(kept, _)| kept == *path))
            .cloned()
            .collect::<Vec<_>>();
        if changed.is_empty() && missing.is_empty() {
            return Ok(None);
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut moved = Vec::new();
        let mut removed = Vec::new();
        for path in &changed {
            let source = root.join(path);
            let parent = source.parent().unwrap_or(root);
            let quarantined = with_owner_access(parent, || {
                crate::agent::state::workspace_state_path(
                    root,
                    &format!("quarantine/{stamp}/{path}"),
                )
                .and_then(|destination| std::fs::rename(&source, destination))
            });
            if quarantined.is_ok() {
                moved.push(path.as_str());
            } else {
                // Such as on a full disk or another file system.
                with_owner_access(parent, || std::fs::remove_file(&source)).map_err(|err| {
                    format!("{path} could not be moved out of the workspace or removed ({err})")
                })?;
                removed.push(path.as_str());
            }
        }
        let mut note = String::new();
        let rule = "a command that runs without review may not create or replace files that host tools run on their own. Write them with the file tools, which ask the user first.";
        if !moved.is_empty() {
            note.push_str(&format!(
                "\nSocium moved {} to .socium/quarantine/{stamp}/: {rule}",
                moved.join(", ")
            ));
        }
        if !removed.is_empty() {
            note.push_str(&format!(
                "\nSocium removed {}, which it could not quarantine: {rule}",
                removed.join(", ")
            ));
        }
        if !missing.is_empty() {
            note.push_str(&format!(
                "\n{} {} no longer where the command found {}.",
                missing.join(", "),
                if missing.len() == 1 { "is" } else { "are" },
                if missing.len() == 1 { "it" } else { "them" },
            ));
        }
        Ok(Some(note))
    }
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

/// The diff an edit call would make, as its approval shows it. The call is
/// planned again after the answer, and refused if the diff changed.
pub(crate) fn edit_review(name: &str, input: &Value, root: &Path) -> String {
    let preview = match name {
        "apply_patch" => crate::tools::apply_patch::preview(root, input).map(Some),
        crate::constants::TEXT_EDITOR_TOOL_NAME => crate::tools::text_editor::preview(root, input),
        _ => Ok(None),
    };
    match preview {
        Ok(Some(diff)) => format!("Edit:\n{diff}"),
        Ok(None) => String::new(),
        Err(err) => format!("Edit: cannot be previewed ({err}); it will fail the same way"),
    }
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
                .filter(|(path, requested)| {
                    approval.protected.matches(path) || approval.protected.matches(requested)
                })
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>();
            if !protected.is_empty() && !granted(&Grant::Edit(protected.clone())) {
                return Ok(Gate::Ask(Ask {
                    reason: format!(
                        "{} {} run or loaded by host tools on their own (direnv, rust-analyzer, editors, CI), outside the sandbox.",
                        protected.join(", "),
                        if protected.len() == 1 { "is" } else { "are" }
                    ),
                    review: Some(edit_review(name, input, root)),
                    grant: Some(Grant::Edit(protected)),
                }));
            }
            if let Some((path, requested)) = read_path(name, input, root)
                && (approval.sensitive.matches(&path) || approval.sensitive.matches(&requested))
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

/// Whether a search or diff of `named` leaves out the contents of `path`,
/// both relative to the workspace: a file that may hold credentials, unless
/// it is the one named, whose read was approved already.
pub(crate) fn withholds(
    sensitive: &crate::settings::PathPatterns,
    path: &str,
    named: &str,
) -> bool {
    path != named && sensitive.matches(path)
}

/// The note under a result that left out such files.
pub(crate) fn withheld_note(paths: &[String]) -> String {
    format!(
        "Left out {} that may hold credentials: {}. Name one as the path to read it, which asks for approval.",
        if paths.len() == 1 { "a file" } else { "files" },
        paths.join(", ")
    )
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

/// Workspace paths (from the root, `/`-separated) a file tool would change,
/// each with the path as it was asked for (see `read_path`).
fn edited_paths(name: &str, input: &Value, root: &Path) -> Vec<(String, String)> {
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
        .filter_map(|path| Some((relative_target(root, path)?, as_requested(root, path))))
        .collect()
}

/// The workspace path a tool would show the contents of, and the path as
/// it was asked for, so a link named like a secret or a protected file is
/// caught wherever it points.
fn read_path(name: &str, input: &Value, root: &Path) -> Option<(String, String)> {
    let path = match name {
        "read_file" | "search_text" | "find_symbol" | "git_diff" => input["path"].as_str()?,
        crate::constants::TEXT_EDITOR_TOOL_NAME if input["command"] == "view" => {
            input["path"].as_str()?
        }
        _ => return None,
    };
    Some((relative_target(root, path)?, as_requested(root, path)))
}

/// `requested` from the root as written, symlinks not followed.
fn as_requested(root: &Path, requested: &str) -> String {
    let requested = Path::new(requested);
    requested
        .strip_prefix(root)
        .unwrap_or(requested)
        .to_string_lossy()
        .replace('\\', "/")
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
        &|path| approval.protected.matches(path) || approval.sensitive.matches(path),
        Listing::Before,
    )?;
    let mut restrictions = Restrictions {
        protected: approval.protected.clone(),
        ..Restrictions::default()
    };
    let mut frozen_dirs: Vec<String> = Vec::new();
    for (file, id) in candidates {
        if approval.sensitive.matches(&file) {
            restrictions.hidden.push(root.join(&file));
        }
        if approval.protected.matches(&file) {
            restrictions.before.insert(file.clone(), id);
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
            // A rooted `/dir/**` names a workspace directory, not a host one.
            let dir = dir
                .strip_prefix("**/")
                .unwrap_or(dir)
                .trim_start_matches('/');
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

/// Runs `op`, and once more after giving the owner full access to `dir`
/// if it was refused: a command can take that access away from a directory
/// it made, to keep its files out of reach.
fn with_owner_access<T>(dir: &Path, op: impl Fn() -> std::io::Result<T>) -> std::io::Result<T> {
    use std::os::unix::fs::PermissionsExt;
    match op() {
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            let mode = std::fs::symlink_metadata(dir)?.permissions().mode();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode | 0o700))?;
            op()
        }
        result => result,
    }
}

fn shallowest_matching_dir(file: &str, patterns: &PathPatterns) -> Option<String> {
    let parts = file.split('/').collect::<Vec<_>>();
    (1..parts.len())
        .map(|end| parts[..end].join("/"))
        .find(|dir| patterns.matches(dir))
}

/// Whether a listing runs before a command or after it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listing {
    /// May give up, since the command then asks instead.
    Before,
    /// Must see all the command left, so it never gives up, and it takes
    /// back the access the command took away from a directory.
    After,
}

/// Workspace files and symlinks whose path `wanted` matches, with their
/// identity, ignored and hidden files included. `.git` and `.socium`
/// directories are skipped, and symlinks are listed, not followed: direnv
/// or Cargo would follow a linked `.envrc` or `build.rs` all the same.
fn find_matching_files(
    root: &Path,
    wanted: &dyn Fn(&str) -> bool,
    listing: Listing,
) -> Result<Vec<(String, FileId)>, String> {
    use std::os::unix::fs::MetadataExt;
    #[cfg(test)]
    let timeout = tests::TIMEOUT.get().unwrap_or(LIST_TIMEOUT);
    #[cfg(not(test))]
    let timeout = LIST_TIMEOUT;
    let deadline = Instant::now() + timeout;
    let unlisted = |err: std::io::Error| format!("protected files could not be listed ({err})");
    // Commands run as the user, so they can read what the user cannot list.
    fn access<T>(
        listing: Listing,
        dir: &Path,
        op: impl Fn() -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        match listing {
            Listing::Before => op(),
            Listing::After => with_owner_access(dir, op),
        }
    }
    let mut found = Vec::new();
    let mut dirs = vec![PathBuf::new()];
    while let Some(dir) = dirs.pop() {
        if listing == Listing::Before && Instant::now() >= deadline {
            return Err("protected files could not be listed in time".into());
        }
        let full = root.join(&dir);
        let entries = access(listing, &full, || std::fs::read_dir(&full)).map_err(unlisted)?;
        for entry in entries {
            let entry = entry.map_err(unlisted)?;
            let relative = dir.join(entry.file_name());
            if entry.file_type().map_err(unlisted)?.is_dir() {
                if entry.file_name() != ".git" && entry.file_name() != ".socium" {
                    dirs.push(relative);
                }
                continue;
            }
            let path = relative.to_string_lossy();
            if wanted(&path) {
                let metadata = access(listing, &full, || entry.metadata()).map_err(unlisted)?;
                found.push((path.into_owned(), (metadata.dev(), metadata.ino())));
            }
        }
    }
    found.sort();
    Ok(found)
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
    fn a_protected_edit_shows_its_diff_for_approval() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("build.rs"), "fn main() {}\n").unwrap();
        let gate = decide(
            root.path(),
            &Settings::default(),
            crate::constants::TEXT_EDITOR_TOOL_NAME,
            json!({"command": "str_replace", "path": "build.rs", "old_str": "fn main() {}", "new_str": "fn main() { run(); }"}),
        );
        let Gate::Ask(ask) = gate else {
            panic!("a protected edit must ask")
        };
        let review = ask.review.unwrap();
        assert!(
            review.contains("-fn main() {}\n+fn main() { run(); }"),
            "{review}"
        );
        assert!(review.contains("build.rs (+1 -1)"), "{review}");
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
        // A new dependency, or a new path crate with a proc macro, runs in
        // rust-analyzer on the host.
        assert!(matches!(edit("Cargo.toml"), Gate::Ask(_)));
        assert!(matches!(edit("crates/x/Cargo.toml"), Gate::Ask(_)));
        // A lock can pin a git dependency to any commit, a fork's included.
        assert!(matches!(edit("Cargo.lock"), Gate::Ask(_)));
        // A link named like a protected file asks too, wherever it points.
        std::fs::write(root.path().join("manifest.toml"), "[package]\n").unwrap();
        std::os::unix::fs::symlink("manifest.toml", root.path().join("Cargo.toml")).unwrap();
        let patch = "*** Begin Patch\n*** Update File: Cargo.toml\n@@\n-[package]\n+[workspace]\n*** End Patch";
        assert!(matches!(
            decide(root.path(), &auto, "apply_patch", json!({"patch": patch})),
            Gate::Ask(_)
        ));
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
                json!({"path": "auth.json"})
            ),
            Gate::Ask(_)
        ));
        // Searching or diffing a named secret shows its contents too.
        for (tool, input) in [
            ("search_text", json!({"pattern": "TOKEN", "path": ".env"})),
            ("git_diff", json!({"path": ".env"})),
            ("find_symbol", json!({"symbol": "KEY", "path": ".env.rs"})),
        ] {
            assert!(
                matches!(decide(root.path(), &auto, tool, input), Gate::Ask(_)),
                "{tool}"
            );
        }
        // A link named like a secret asks too, wherever it points.
        std::fs::create_dir(root.path().join("secrets")).unwrap();
        std::fs::write(root.path().join("secrets/token"), "x").unwrap();
        std::os::unix::fs::symlink("secrets/token", root.path().join("credentials.json")).unwrap();
        assert!(matches!(
            decide(
                root.path(),
                &auto,
                "read_file",
                json!({"path": "credentials.json"})
            ),
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

    thread_local! {
        /// Replaces `LIST_TIMEOUT`, so a test need not fill a workspace.
        pub(super) static TIMEOUT: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
    }

    fn auto() -> Settings {
        Settings::from_toml("[approval]\ncommands = \"auto\"\n", None)
            .unwrap()
            .0
    }

    /// Commands run as the user, so a directory nobody can list still lets
    /// them read a file whose name they know.
    #[test]
    fn a_directory_that_cannot_be_listed_makes_a_command_ask() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let locked = root.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join(".env"), "TOKEN=x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o111)).unwrap();
        let gate = decide(
            root.path(),
            &auto(),
            "run_command",
            json!({"command": ["cat", "locked/.env"]}),
        );
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(gate, Gate::Ask(_)));
    }

    /// Giving up on the listing after a command would leave what it wrote.
    #[test]
    fn only_the_listing_before_a_command_gives_up_in_time() {
        let root = tempfile::tempdir().unwrap();
        let settings = auto();
        let restrictions = restrictions(root.path(), &settings.approval).unwrap();
        TIMEOUT.set(Some(Duration::ZERO));
        let gate = decide(
            root.path(),
            &settings,
            "run_command",
            json!({"command": ["true"]}),
        );
        std::fs::write(root.path().join(".envrc"), "x").unwrap();
        let withheld = restrictions.withhold_changes(root.path());
        TIMEOUT.set(None);
        assert!(matches!(gate, Gate::Ask(_)));
        assert!(withheld.unwrap().unwrap().contains(".envrc"));
        assert!(root.path().join(".envrc").symlink_metadata().is_err());
    }

    #[test]
    fn unreviewed_commands_lose_secrets_and_host_run_files() {
        let root = tempfile::tempdir().unwrap();
        for file in [
            ".env",
            "app/.env.local",
            "backend/auth.json",
            ".github/workflows/ci.yaml",
            "build.rs",
            "crates/x/Cargo.toml",
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
        let mut hidden = names(&restrictions.hidden);
        hidden.sort();
        assert_eq!(hidden, [".env", "app/.env.local", "backend/auth.json"]);
        let mut frozen = names(&restrictions.frozen);
        frozen.sort();
        assert_eq!(
            frozen,
            [
                ".github/workflows",
                ".vscode",
                "build.rs",
                "crates/x/Cargo.toml"
            ]
        );
    }

    mod properties {
        use super::super::*;
        use crate::settings::ApprovalSettings;
        use proptest::prelude::*;
        use serde_json::json;

        const SHELLS_AND_WRAPPERS: &[&str] = &[
            "sh",
            "/bin/sh",
            "bash",
            "env",
            "/usr/bin/env",
            "xargs",
            "timeout",
            "nix",
        ];

        fn word() -> impl Strategy<Value = String> {
            prop::sample::select(
                [
                    "cargo",
                    "test",
                    "build",
                    "git",
                    "push",
                    "status",
                    "rm",
                    "-rf",
                    ".",
                    "./cargo",
                    "/usr/bin/cargo",
                    "python3",
                    "-m",
                    "pytest",
                    "-c",
                    "make",
                    "npm",
                    "run",
                    "a=b",
                    "x/y",
                    "",
                ]
                .iter()
                .chain(SHELLS_AND_WRAPPERS)
                .map(|word| word.to_string())
                .collect::<Vec<_>>(),
            )
        }

        fn argv() -> impl Strategy<Value = Vec<String>> {
            prop::collection::vec(word(), 1..5)
        }

        /// A rule close to `argv`: one of its prefixes, possibly one word too
        /// long or with its last word changed, so that matches and near
        /// misses are both common.
        fn near(argv: Vec<String>) -> impl Strategy<Value = Vec<String>> {
            (1..=argv.len() + 1, any::<bool>(), word()).prop_map(move |(length, change, other)| {
                let mut rule = argv.iter().take(length).cloned().collect::<Vec<_>>();
                if length > argv.len() {
                    rule.push(other);
                } else if change {
                    *rule.last_mut().unwrap() = other;
                }
                rule
            })
        }

        fn rules() -> impl Strategy<Value = Vec<Vec<String>>> {
            prop::collection::vec(prop::collection::vec(word(), 1..3), 0..3)
        }

        fn settings(auto: bool, allow: Vec<Vec<String>>, deny: Vec<Vec<String>>) -> Settings {
            Settings {
                approval: ApprovalSettings {
                    commands: if auto {
                        CommandApproval::Auto
                    } else {
                        CommandApproval::Ask
                    },
                    allow,
                    deny,
                    ..ApprovalSettings::default()
                },
                ..Settings::default()
            }
        }

        /// A workspace holding one secret and one file host tools run.
        fn workspace() -> tempfile::TempDir {
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join(".env"), "TOKEN=fake\n").unwrap();
            std::fs::write(root.path().join("build.rs"), "fn main() {}\n").unwrap();
            root
        }

        proptest! {
            // Each case lists the workspace with ripgrep.
            #![proptest_config(ProptestConfig::with_cases(64))]

            /// What runs unasked follows only from config rules, session grants and auto mode, and deny always wins.
            #[test]
            fn a_command_runs_unasked_only_when_the_user_allowed_it(
                (argv, allow, deny, grant) in argv().prop_flat_map(|argv| {
                    let rules = || prop::collection::vec(near(argv.clone()), 0..3);
                    (Just(argv.clone()), rules(), rules(), prop::option::of(near(argv.clone())))
                }),
                auto in any::<bool>(),
            ) {
                let root = workspace();
                let grants = grant.clone().map(Grant::Command).into_iter().collect::<Vec<_>>();
                let settings = settings(auto, allow.clone(), deny.clone());
                let gate = gate("run_command", &json!({"command": argv}), root.path(), &settings, &|wanted| grants_cover(&grants, wanted)).unwrap();
                let denied = deny.iter().any(|rule| argv.starts_with(rule));
                let allowed = auto
                    || allow.iter().any(|rule| argv.starts_with(rule))
                    || grant.as_ref().is_some_and(|prefix| argv.starts_with(prefix));
                match gate {
                    Gate::Deny(_) => prop_assert!(denied),
                    Gate::Run(_) => prop_assert!(!denied && allowed),
                    Gate::Ask(_) => prop_assert!(!denied && !allowed),
                }
            }

            /// A command nobody reviewed never reads the workspace's secrets or changes files host tools run.
            #[test]
            fn an_unreviewed_command_always_runs_restricted(argv in argv(), allow in rules()) {
                let root = workspace();
                let settings = settings(true, allow, Vec::new());
                let gate = gate("run_command", &json!({"command": argv}), root.path(), &settings, &|_| false).unwrap();
                let Gate::Run(Some(restrictions)) = gate else {
                    return Err(TestCaseError::fail("an auto-approved command must run with restrictions"));
                };
                prop_assert_eq!(restrictions.hidden, vec![root.path().join(".env")]);
                prop_assert_eq!(restrictions.frozen, vec![root.path().join("build.rs")]);
            }

            /// "Allow for this session" offers only a prefix of the asked command and never a shell or wrapper.
            #[test]
            fn a_session_offer_is_a_prefix_and_never_a_wrapper(argv in argv()) {
                let Some(prefix) = suggest_prefix(&argv) else { return Ok(()) };
                prop_assert!(argv.starts_with(&prefix));
                let name = Path::new(&prefix[0]).file_name().and_then(|name| name.to_str()).unwrap_or_default();
                prop_assert!(!SHELLS_AND_WRAPPERS.iter().any(|wrapper| wrapper.ends_with(name)), "{:?}", prefix);
                for word in &prefix[1..] {
                    prop_assert!(word == "-m" || (!word.is_empty() && !word.starts_with('-') && !word.contains(['/', '=', '.'])), "{:?}", prefix);
                }
            }

            /// Edits to host-run files and reads of secrets ask in every mode, whatever command rules exist.
            #[test]
            fn protected_edits_and_sensitive_reads_always_ask(
                auto in any::<bool>(), allow in rules(),
                grant in prop::option::of(prop::collection::vec(word(), 1..3)),
                edited in prop::sample::select(vec!["build.rs", "crates/x/build.rs", ".envrc", ".github/workflows/ci.yaml", "a/.vscode/settings.json", ".cargo/config.toml", "./build.rs"]),
                read in prop::sample::select(vec![".env", "app/.env.local", "id_rsa", "certs/x.pem", "./.env", ".npmrc"]),
            ) {
                let root = workspace();
                let grants = grant.map(Grant::Command).into_iter().collect::<Vec<_>>();
                let settings = settings(auto, allow, Vec::new());
                let granted = |wanted: &Grant| grants_cover(&grants, wanted);
                let edit = gate(crate::constants::TEXT_EDITOR_TOOL_NAME, &json!({"command": "create", "path": edited, "file_text": "x"}), root.path(), &settings, &granted).unwrap();
                prop_assert!(matches!(edit, Gate::Ask(Ask { grant: Some(Grant::Edit(_)), .. })), "edit {edited}");
                let decision = gate("read_file", &json!({"path": read}), root.path(), &settings, &granted).unwrap();
                prop_assert!(matches!(decision, Gate::Ask(Ask { grant: Some(Grant::Read(_)), .. })), "read {read}");
            }
        }
    }

    #[test]
    fn patterns_written_from_the_root_with_dot_slash_still_protect() {
        let root = tempfile::tempdir().unwrap();
        for file in ["scripts/deploy.sh", "sub/scripts/deploy.sh", "tools/run"] {
            let path = root.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        // An empty directory is frozen from the pattern alone.
        std::fs::create_dir(root.path().join("hooks")).unwrap();
        let (settings, _) = Settings::from_toml(
            "[approval]\nprotected = [\"./scripts/deploy.sh\", \"./tools/**\", \"./hooks/**\"]",
            None,
        )
        .unwrap();
        let edit = |path: &str| {
            decide(
                root.path(),
                &settings,
                crate::constants::TEXT_EDITOR_TOOL_NAME,
                json!({"command": "create", "path": path, "file_text": "y"}),
            )
        };
        assert!(matches!(edit("scripts/deploy.sh"), Gate::Ask(_)));
        assert!(matches!(edit("tools/new"), Gate::Ask(_)));
        assert!(matches!(edit("sub/scripts/deploy.sh"), Gate::Run(None)));
        let restrictions = restrictions(root.path(), &settings.approval).unwrap();
        let mut frozen = restrictions
            .frozen
            .iter()
            .map(|path| path.strip_prefix(root.path()).unwrap().to_path_buf())
            .collect::<Vec<_>>();
        frozen.sort();
        assert_eq!(
            frozen,
            [
                PathBuf::from("hooks"),
                PathBuf::from("scripts/deploy.sh"),
                PathBuf::from("tools")
            ]
        );
    }
}
