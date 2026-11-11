/// Error type for provider transport and streaming.
///
/// Display output is user-visible; never include credentials or raw HTTP bodies.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
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
