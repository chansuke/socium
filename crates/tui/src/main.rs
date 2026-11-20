mod activity;
mod agent_task;
mod app;
mod approval;
mod cli;
mod client_bridge;
mod headless;
mod input_history;
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
use cli::Launch;
use color_eyre::{Result, eyre::eyre};
use socium_core::{AgentService, SessionStart};
use std::{env, path::Path};

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
