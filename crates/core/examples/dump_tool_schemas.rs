fn main() {
    let tools = socium_core::tools::default_tools();
    println!("{}", serde_json::to_string_pretty(&tools).unwrap());
}
