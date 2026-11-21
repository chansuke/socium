pub mod compaction;
pub mod context;
pub(crate) mod subagent;
pub mod turn;

use crate::conversation::ToolDefinition;

/// The tools a model is offered with default settings and the built-in
/// subagent types.
pub fn default_tools() -> Vec<ToolDefinition> {
    tools_for_model()
}

pub(crate) fn tools_for_model() -> Vec<ToolDefinition> {
    crate::tools::tools_with_settings(
        &crate::settings::Settings::default(),
        subagent::catalog::SubagentCatalog::builtin().tool_definition(),
    )
}
