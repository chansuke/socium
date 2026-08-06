/// Error type for the tool runtime.
///
/// Invariant: every `Display` output must stay byte-identical to the ad-hoc
/// strings it replaced, because tool errors are surfaced verbatim to the
/// model as tool results and asserted by tests.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ToolError {
    #[error("Unsupported tool: {0}")]
    UnsupportedTool(String),
    #[error("{tool} requires an object input")]
    NotAnObject { tool: String },
    #[error("{context} requires a string field: {field}")]
    MissingStringField { context: String, field: String },
    #[error("{context} requires a boolean field: {field}")]
    MissingBoolField { context: String, field: String },
    #[error("{context} requires an integer field: {field}")]
    MissingIntegerField { context: String, field: String },
    #[error("{context} requires an array field: {field}")]
    MissingArrayField { context: String, field: String },
    #[error("{context} requires a non-empty array field: {field}")]
    EmptyArrayField { context: String, field: String },
    #[error("{context} requires string array items in field: {field}")]
    NonStringArrayItem { context: String, field: String },
    #[error("{context} field '{field}' is too large for this platform")]
    IntegerFieldTooLarge { context: String, field: String },
    #[error("{context} field '{field}' must be between 1 and {max}")]
    IntegerFieldOutOfRange {
        context: String,
        field: String,
        max: usize,
    },
    #[error("Absolute paths are not allowed: '{path}'")]
    AbsolutePath { path: String },
    #[error("Path '{path}' is outside the workspace")]
    OutsideWorkspace { path: String },
    /// Catch-all for one-off formatted messages that have not been lifted
    /// into a typed variant.
    #[error("{0}")]
    Message(String),
}

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for ToolError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_string())
    }
}
