mod activity;
mod agent_task;
mod app;
mod approval;
mod cli;
mod input_history;
mod model_picker;
mod progress;
mod session_picker;
mod slash_command;
mod transcript;
mod view;

use app::App;
use cli::Launch;
use color_eyre::{Result, eyre::eyre};
use session_picker::SessionPicker;
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
    let launch = match cli::parse(args) {
        Ok(Launch::Help) => {
            println!("{}", cli::usage(&program));
            return Ok(());
        }
        Ok(launch) => launch,
        Err(message) => {
            eprintln!("{message}\n\n{}", cli::usage(&program));
            std::process::exit(2);
        }
    };

    // The resume picker needs the terminal before the agent starts.
    let mut terminal = None;
    let start = match launch {
        Launch::New | Launch::Help => SessionStart::New,
        Launch::Continue => SessionStart::Continue,
        Launch::Resume(Some(id)) => SessionStart::Resume(id),
        Launch::Resume(None) => {
            let sessions = socium_core::list_sessions(&env::current_dir()?)
                .map_err(|err| eyre!(err.to_string()))?;
            if sessions.is_empty() {
                eprintln!("No saved conversation to resume in this workspace");
                return Ok(());
            }
            let mut picker_terminal = ratatui::init();
            match SessionPicker::new(sessions).run(&mut picker_terminal) {
                Ok(Some(id)) => {
                    terminal = Some(picker_terminal);
                    SessionStart::Resume(id)
                }
                chosen => {
                    ratatui::restore();
                    return chosen.map(|_| ());
                }
            }
        }
    };

    let app = AgentService::from_current_dir(start)
        .map_err(|err| eyre!(err.to_string()))
        .and_then(App::new);
    let app = match app {
        Ok(app) => app,
        Err(err) => {
            if terminal.is_some() {
                ratatui::restore();
            }
            return Err(err);
        }
    };
    let terminal = terminal.unwrap_or_else(ratatui::init);
    // Ratatui's global panic hook restores the terminal. Worker panics are
    // caught and reported through the task channel, so leave the UI running.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() != Some("socium-worker") {
            previous_hook(info);
        }
    }));
    let app_result = app.run(terminal).await;
    ratatui::restore();
    if let Ok(Some(id)) = &app_result {
        println!("Resume this conversation with: {program} -r {id}");
    }
    app_result.map(|_| ())
}
