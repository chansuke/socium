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
    let terminal = ratatui::init();
    let app_result = App::new(agent).run(terminal).await;
    ratatui::restore();
    app_result
}
