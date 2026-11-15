mod activity;
mod agent_task;
mod app;
mod approval;
mod cli;
mod headless;
mod input_history;
mod model_picker;
mod progress;
mod session_picker;
mod slash_command;
mod subagents;
mod transcript;
mod view;

use app::App;
use cli::Launch;
use color_eyre::{Result, eyre::eyre};
use socium_core::{AgentService, SessionStart};
use std::{env, path::Path};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let mut args = env::args();
    let program = args
        .next()
        .and_then(|arg| Some(Path::new(&arg).file_name()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "socium-tui".to_string());
    let cli::Args { launch, print } = match cli::parse(args) {
        Ok(cli::Args {
            launch: Launch::Help,
            ..
        }) => {
            println!("{}", cli::usage(&program));
            return Ok(());
        }
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n\n{}", cli::usage(&program));
            std::process::exit(2);
        }
    };

    let (start, pick_session) = match launch {
        Launch::New | Launch::Help => (SessionStart::New, false),
        Launch::Continue => (SessionStart::Continue, false),
        Launch::Resume(Some(id)) => (SessionStart::Resume(id), false),
        // A new session opens under the picker; closing it keeps that one.
        Launch::Resume(None) => (SessionStart::New, true),
    };
    if let Some(prompt) = print {
        std::process::exit(headless::main(start, &prompt).await);
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
