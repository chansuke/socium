/// Error type for provider transport and streaming.
///
/// Display output is user-visible; never include credentials or raw HTTP bodies.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// reqwest's own message names only the step, such as "error sending
    /// request"; the innermost cause says what went wrong.
    #[error("{}", with_cause(.0))]
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
    /// A non-2xx response; `message` is already fully formatted.
    #[error("{message}")]
    Status {
        status: u16,
        /// From the `retry-after` header, in seconds.
        retry_after: Option<std::time::Duration>,
        message: String,
    },
    /// An overload or internal error the provider reported in the stream.
    #[error("{0}")]
    Transient(String),
    /// No data arrived for the idle limit.
    #[error("No data from the provider for {} seconds; the connection was closed", .0.as_secs())]
    Stalled(std::time::Duration),
}

fn with_cause(err: &reqwest::Error) -> String {
    let mut cause: &dyn std::error::Error = err;
    while let Some(source) = cause.source() {
        cause = source;
    }
    if std::ptr::addr_eq(cause, err) {
        err.to_string()
    } else {
        format!("{err}: {cause}")
    }
}

/// How long the provider may send nothing before the request is given up.
/// Long responses are fine as long as data keeps arriving.
pub(crate) const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

impl ProviderError {
    /// Whether another attempt can succeed: rate limits, overload, server
    /// errors, and connections that failed, stalled, or closed before the
    /// response, as a server does to a kept-alive connection it already
    /// gave up. The caller retries only before any output was shown.
    pub(crate) fn is_retryable(&self) -> bool {
        match self {
            Self::Status { status, .. } => {
                matches!(status, 408 | 409 | 425 | 429 | 500 | 502 | 503 | 504 | 529)
            }
            Self::Transient(_) | Self::Stalled(_) => true,
            Self::Http(err) => err.is_connect() || err.is_timeout() || err.is_request(),
            _ => false,
        }
    }

    pub(crate) fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            Self::Status { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

/// A transport error, with a stall named as such rather than as a body
/// decoding failure.
pub(crate) fn transport(err: reqwest::Error) -> ProviderError {
    if err.is_timeout() {
        ProviderError::Stalled(IDLE_TIMEOUT)
    } else {
        ProviderError::Http(err)
    }
}

const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;
const MAX_ERROR_MESSAGE_CHARS: usize = 300;

/// Reads a bounded prefix of a non-2xx response and formats it.
pub(crate) async fn api_error(provider: &str, mut response: reqwest::Response) -> ProviderError {
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(std::time::Duration::from_secs);
    let mut body = Vec::new();
    while body.len() < MAX_ERROR_BODY_BYTES
        && let Ok(Some(chunk)) = response.chunk().await
    {
        body.extend_from_slice(&chunk);
    }
    body.truncate(MAX_ERROR_BODY_BYTES);
    ProviderError::Status {
        status: status.as_u16(),
        retry_after,
        message: api_error_message(provider, status, &body),
    }
}

/// Formats a non-2xx response for the UI and persisted transcript. Only the
/// documented error fields are shown, bounded and stripped of control
/// characters, so the raw body (which may echo request text) and terminal
/// escape sequences never reach the screen.
pub(crate) fn api_error_message(
    provider: &str,
    status: reqwest::StatusCode,
    body: &[u8],
) -> String {
    let fallback = format!("{provider} API error {status}");
    match serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| describe_error(&value["error"]))
    {
        Some(detail) => format!("{fallback}: {detail}"),
        None => fallback,
    }
}

/// `kind: message` from an error object: Anthropic's `type`, Google's
/// `status` (such as `RESOURCE_EXHAUSTED`), or the `code` that
/// OpenAI-compatible servers send when neither is present.
pub(crate) fn describe_error(error: &serde_json::Value) -> Option<String> {
    let message = error["message"].as_str()?;
    let mut cleaned = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c });
    let mut shown = cleaned
        .by_ref()
        .take(MAX_ERROR_MESSAGE_CHARS)
        .collect::<String>();
    if cleaned.next().is_some() {
        shown.push('…');
    }
    let kind = error["type"]
        .as_str()
        .or_else(|| error["status"].as_str())
        .map(str::to_string)
        .or_else(|| match &error["code"] {
            serde_json::Value::String(code) => Some(code.clone()),
            serde_json::Value::Number(code) => Some(code.to_string()),
            _ => None,
        })
        .map(|kind| {
            kind.chars()
                .filter(|c| !c.is_control())
                .take(64)
                .collect::<String>()
        });
    Some(match kind {
        Some(kind) => format!("{kind}: {shown}"),
        None => shown,
    })
}

#[cfg(test)]
mod tests {
    use super::api_error_message;
    use reqwest::StatusCode;

    #[test]
    fn api_error_shows_type_and_message() {
        let body = br#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."},"request_id":"req_1"}"#;
        assert_eq!(
            api_error_message("anthropic", StatusCode::BAD_REQUEST, body),
            "anthropic API error 400 Bad Request: invalid_request_error: Your credit balance is too low to access the Anthropic API."
        );
    }

    #[test]
    fn api_error_falls_back_to_the_code_then_the_message_alone() {
        let zai = br#"{"error":{"code":"1000","message":"auth failed"}}"#;
        assert_eq!(
            api_error_message("zai", StatusCode::UNAUTHORIZED, zai),
            "zai API error 401 Unauthorized: 1000: auth failed"
        );
        let openai = br#"{"error":{"message":"No such model","type":null,"code":404}}"#;
        assert_eq!(
            api_error_message("openai", StatusCode::NOT_FOUND, openai),
            "openai API error 404 Not Found: 404: No such model"
        );
        let google =
            br#"{"error":{"code":429,"message":"Quota exceeded","status":"RESOURCE_EXHAUSTED"}}"#;
        assert_eq!(
            api_error_message("gemini", StatusCode::TOO_MANY_REQUESTS, google),
            "gemini API error 429 Too Many Requests: RESOURCE_EXHAUSTED: Quota exceeded"
        );
        let bare = br#"{"error":{"message":"busy"}}"#;
        assert_eq!(
            api_error_message("local", StatusCode::SERVICE_UNAVAILABLE, bare),
            "local API error 503 Service Unavailable: busy"
        );
    }

    #[test]
    fn api_error_falls_back_to_status_for_unrecognized_bodies() {
        for body in [
            &b"<html>bad gateway</html>"[..],
            b"",
            br#"{"error":{"type":"x""#,
            br#"{"error":"plain string"}"#,
        ] {
            assert_eq!(
                api_error_message("deepseek", StatusCode::BAD_GATEWAY, body),
                "deepseek API error 502 Bad Gateway"
            );
        }
    }

    #[test]
    fn api_error_message_is_bounded_and_strips_control_characters() {
        let message = format!("bad\u{1b}[31m input\n{}", "x".repeat(1000));
        let body =
            serde_json::json!({"error": {"type": "invalid_request_error", "message": message}});
        let shown = api_error_message(
            "anthropic",
            StatusCode::BAD_REQUEST,
            body.to_string().as_bytes(),
        );
        assert!(!shown.chars().any(char::is_control));
        assert!(shown.contains("bad [31m input x"));
        assert!(shown.ends_with('…'));
        assert!(shown.chars().count() < 400);
    }
}
