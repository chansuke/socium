mod activity;
mod agent_task;
mod app;
mod approval;
mod cli;
mod headless;
mod input_history;
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
    if let Ok(mut child) = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || child.wait());
    }
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
