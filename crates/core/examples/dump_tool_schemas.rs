#[expect(clippy::print_stdout, reason = "the example prints the schemas")]
fn main() -> serde_json::Result<()> {
    let tools = socium_core::agent::default_tools();
    println!("{}", serde_json::to_string_pretty(&tools)?);
    Ok(())
}
