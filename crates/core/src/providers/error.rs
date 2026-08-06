/// Error type for provider transport and streaming.
///
/// Invariant: `Display` output must stay identical to the strings the
/// previous `Box<dyn Error>`/`io::Error` values produced, because the TUI
/// surfaces provider failures via `to_string()`.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("Provider '{0}' is not implemented yet")]
    NotImplemented(&'static str),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Utf8(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Env(#[from] std::io::Error),
    /// Non-2xx API responses and in-stream error events; the message is
    /// already fully formatted.
    #[error("{0}")]
    Api(String),
    /// SSE framing / stream-state errors; the message is already fully
    /// formatted.
    #[error("{0}")]
    Stream(String),
}
