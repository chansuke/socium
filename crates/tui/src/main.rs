mod activity;
mod agent_task;
mod app;
mod approval;
mod cli;
mod headless;
mod input_history;
mod mcp_status;
mod model_picker;
mod pane_slot;
mod progress;
mod rewind_picker;
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
/// Socium's environment; the rest, provider keys included, stays out.
const DESKTOP_VARIABLES: &[&str] = &[
    "HOME",
    "USER",
    "LANG",
    "BROWSER",
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
    let path = trusted_path(&env::var("PATH").unwrap_or_default(), &workspace);
    // Found on a PATH without the workspace, so the project cannot provide
    // the program that runs with the user's desktop session.
    let Some(program) = env::split_paths(&path)
        .map(|dir| dir.join(opener))
        .find(|candidate| candidate.is_file())
    else {
        return;
    };
    let mut command = std::process::Command::new(program);
    command
        .arg(url)
        .current_dir("/")
        .env_clear()
        .env("PATH", &path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for variable in DESKTOP_VARIABLES {
        if let Ok(value) = env::var(variable) {
            command.env(variable, value);
        }
    }
    if let Ok(mut child) = command.spawn() {
        std::thread::spawn(move || child.wait());
    }
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

#[tokio::main(worker_threads = 1)]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let mut args = env::args();
    let program = args
        .next()
        .and_then(|arg| Some(Path::new(&arg).file_name()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "socium".to_string());
    let cli::Args { launch, print } = match cli::parse(args) {
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
        Launch::Mcp(command) => std::process::exit(mcp(command)),
        Launch::New | Launch::Help | Launch::Version => (SessionStart::New, false),
        Launch::Continue => (SessionStart::Continue, false),
        Launch::Resume(Some(id)) => (SessionStart::Resume(id), false),
        // A new session opens under the picker; closing it keeps that one.
        Launch::Resume(None) => (SessionStart::New, true),
    };
    if let Some(print) = print {
        std::process::exit(headless::main(start, print).await);
    }
    let agent = AgentService::from_current_dir(start).map_err(|err| eyre!(err.to_string()))?;
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
}
