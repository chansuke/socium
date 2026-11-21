mod activity;
mod agent_task;
mod app;
mod approval;
mod cli;
mod client_bridge;
mod headless;
mod input_history;
mod mcp_status;
mod model_picker;
mod models_command;
mod pane_slot;
mod progress;
mod rewind_picker;
#[cfg(test)]
mod scenario;
mod session_picker;
mod slash_command;
mod subagents;
mod timeline;
mod transcript;
mod view;

use app::App;
use cli::{Launch, McpCommand};
use color_eyre::{Result, eyre::eyre};
use socium_core::{AgentService, SessionStart};
use std::{env, path::Path};

/// Runs `socium mcp ...` and returns the exit status.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "socium mcp reports on stdout and stderr and has no terminal UI"
)]
fn mcp(command: McpCommand) -> i32 {
    let result = match &command {
        McpCommand::Login(server) => {
            socium_core::mcp_login(server, &|url| open_in_browser(server, url))
        }
        McpCommand::Logout(server) => socium_core::mcp_logout(server),
    };
    match result {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err(message) => {
            eprintln!("{message}");
            1
        }
    }
}

/// What the desktop's opener and the browser it starts need from
/// Socium's environment; the rest, provider keys included, stays out. So
/// does `BROWSER`, which xdg-open would run in place of the desktop's
/// browser, and which may name a program in the workspace.
const DESKTOP_VARIABLES: &[&str] = &[
    "HOME",
    "USER",
    "LANG",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "DBUS_SESSION_BUS_ADDRESS",
];

/// Shows the authorization URL and tries the desktop's opener; a missing
/// opener leaves the URL to copy.
#[expect(
    clippy::print_stderr,
    reason = "the URL is shown on stderr, outside the terminal UI, so stdout stays the command's result"
)]
fn open_in_browser(server: &str, url: &str) {
    eprintln!(
        "To log in to MCP server {server}, open this URL in a browser:\n\n  {url}\n\nWaiting for the browser to come back (up to 5 minutes)..."
    );
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let workspace = env::current_dir().unwrap_or_default();
    let path = trusted_path(
        &socium_core::host::process_env("PATH").unwrap_or_default(),
        &workspace,
    );
    let Some(program) = find_opener(opener, &path, &workspace) else {
        return;
    };
    let mut command = opener_command(&program, url, &path, &socium_core::host::process_env);
    if let Ok(mut child) = command.spawn() {
        std::thread::spawn(move || child.wait());
    }
}

/// `opener` on `path`, a PATH without the workspace, unless the file found
/// is a link into the workspace: the project must not provide the program
/// that runs with the user's desktop session.
fn find_opener(opener: &str, path: &str, workspace: &Path) -> Option<std::path::PathBuf> {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let found = env::split_paths(path)
        .map(|dir| dir.join(opener))
        .find(|candidate| candidate.is_file())?;
    let resolved = found.canonicalize().ok()?;
    (!resolved.starts_with(&workspace)).then_some(found)
}

/// The opener's command: in `/`, with `path` and the desktop's variables
/// of `lookup` only.
fn opener_command(
    program: &Path,
    url: &str,
    path: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command
        .arg(url)
        .current_dir("/")
        .env_clear()
        .env("PATH", path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for variable in DESKTOP_VARIABLES {
        if let Some(value) = lookup(variable) {
            command.env(variable, value);
        }
    }
    command
}

/// `path` without relative entries and entries in `workspace`.
fn trusted_path(path: &str, workspace: &Path) -> String {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let kept = env::split_paths(path).filter(|entry| {
        let resolved = entry.canonicalize().unwrap_or_else(|_| entry.clone());
        entry.is_absolute() && !resolved.starts_with(&workspace) && !entry.starts_with(&workspace)
    });
    env::join_paths(kept)
        .map(|joined| joined.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "usage, version and the resume hint print outside the terminal UI"
)]
#[tokio::main(worker_threads = 1)]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let mut args = env::args();
    let program = args
        .next()
        .and_then(|arg| Some(Path::new(&arg).file_name()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "socium".to_string());
    let args = args.collect::<Vec<_>>();
    if args == ["--client-bridge"] {
        return client_bridge::run().await;
    }
    let cli::Args {
        launch,
        print,
        skip_permissions,
    } = match cli::parse(args) {
        Ok(cli::Args {
            launch: Launch::Help,
            ..
        }) => {
            println!("{}", cli::usage(&program));
            return Ok(());
        }
        Ok(cli::Args {
            launch: Launch::Version,
            ..
        }) => {
            println!("socium {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n\n{}", cli::usage(&program));
            std::process::exit(2);
        }
    };

    let (start, pick_session) = match launch {
        Launch::ModelsImport(command) => std::process::exit(models_command::main(command).await),
        Launch::Mcp(command) => std::process::exit(mcp(command)),
        Launch::New | Launch::Help | Launch::Version => (SessionStart::New, false),
        Launch::Continue => (SessionStart::Continue, false),
        Launch::Resume(Some(id)) => (SessionStart::Resume(id), false),
        // A new session opens under the picker; closing it keeps that one.
        Launch::Resume(None) => (SessionStart::New, true),
    };
    if let Some(print) = print {
        std::process::exit(headless::main(start, print, skip_permissions).await);
    }
    let mut agent = AgentService::from_current_dir(start).map_err(|err| eyre!(err.to_string()))?;
    if skip_permissions {
        agent.skip_approvals();
    }
    let mut app = App::new(agent)?;
    if pick_session {
        app.open_session_picker();
    }
    let terminal = ratatui::init();
    // Pasted text arrives whole, newlines included, instead of as keys
    // where the first newline would send the message.
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableBracketedPaste
    );
    // Ratatui's global panic hook restores the terminal. Worker panics are
    // caught and reported through the task channel, so leave the UI running.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() != Some("socium-worker") {
            previous_hook(info);
        }
    }));
    let app_result = app.run(terminal).await;
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableBracketedPaste
    );
    ratatui::restore();
    if let Ok(Some(id)) = &app_result {
        println!("Resume this conversation with: {program} -r {id}");
    }
    app_result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opener_is_looked_up_on_a_path_without_the_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let bin = workspace.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let path = format!(".:bin:{}:/usr/bin:/bin", bin.display());

        assert_eq!(trusted_path(&path, workspace.path()), "/usr/bin:/bin");
    }

    #[cfg(unix)]
    #[test]
    fn an_opener_that_leads_into_the_workspace_is_not_run() {
        use std::os::unix::fs::symlink;
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let planted = workspace.path().join("xdg-open");
        std::fs::write(&planted, "#!/bin/sh\n").unwrap();
        // A PATH directory outside that links into the workspace.
        let linked_dir = outside.path().join("linked-dir");
        symlink(workspace.path(), &linked_dir).unwrap();
        let path = trusted_path(&linked_dir.display().to_string(), workspace.path());
        assert_eq!(find_opener("xdg-open", &path, workspace.path()), None);
        // An opener outside that links to a program in the workspace.
        let bin = outside.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        symlink(&planted, bin.join("xdg-open")).unwrap();
        let path = trusted_path(&bin.display().to_string(), workspace.path());
        assert_eq!(path, bin.display().to_string());
        assert_eq!(find_opener("xdg-open", &path, workspace.path()), None);
        // A real one outside is found.
        let real = outside.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("xdg-open"), "#!/bin/sh\n").unwrap();
        let path = format!("{}:{}", bin.display(), real.display());
        assert_eq!(
            find_opener("xdg-open", &path, workspace.path()),
            None,
            "the first opener on PATH links into the workspace"
        );
        let path = real.display().to_string();
        assert_eq!(
            find_opener("xdg-open", &path, workspace.path()),
            Some(real.join("xdg-open"))
        );
    }

    #[test]
    fn the_opener_gets_neither_browser_nor_provider_keys() {
        let lookup = |name: &str| match name {
            "BROWSER" => Some("/workspace/evil".to_string()),
            "ANTHROPIC_API_KEY" => Some("sk-ant-must-not-leak".to_string()),
            "OPENAI_API_KEY" => Some("sk-must-not-leak".to_string()),
            "DISPLAY" => Some(":0".to_string()),
            _ => None,
        };

        let command = opener_command(
            Path::new("/usr/bin/xdg-open"),
            "https://auth.example/authorize",
            "/usr/bin",
            &lookup,
        );

        let envs = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            envs,
            [
                ("DISPLAY".to_string(), Some(":0".to_string())),
                ("PATH".to_string(), Some("/usr/bin".to_string())),
            ]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/")));
    }
}
