mod agent_task;
mod app;
mod input_history;
mod progress;
mod transcript;
mod view;

use app::App;
use color_eyre::{Result, eyre::eyre};
use socium_core::AgentService;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let agent = AgentService::from_current_dir().map_err(|err| eyre!(err.to_string()))?;
    let app = App::new(agent)?;
    let terminal = ratatui::init();
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
    app_result
}
