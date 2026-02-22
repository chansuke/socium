#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    socium_core::run_cli().await
}
