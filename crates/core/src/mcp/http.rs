//! The Streamable HTTP transport (MCP 2025-06-18, ADR 21): every message
//! is a POST to the server's URL, and a request's response comes back as
//! one JSON body or in an event stream.
//!
//! reqwest is async, and a runtime must not be dropped inside another, so
//! each server gets a thread that runs its own current-thread runtime.
//! Callers hand it commands over a channel and wait on std channels, which
//! keeps the client synchronous like the stdio transport.
//!
//! The configured headers usually hold a token: they are marked sensitive,
//! and the client blanks their values out of the messages it builds from
//! what the server answers.

use super::{MAX_MESSAGE_BYTES, Reply, RequestError, lock};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Response, StatusCode, Url};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

const SESSION_HEADER: &str = "mcp-session-id";
const VERSION_HEADER: &str = "mcp-protocol-version";
/// The connection must open within this long; a call itself may run until
/// the user cancels it, as over stdio.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long ending the session may take when the server is dropped.
const END_SESSION_TIMEOUT: Duration = Duration::from_secs(2);
/// A JSON-RPC error message quoted from an HTTP error body is cut here.
const QUOTED_ERROR_CHARS: usize = 300;
/// Answers to the server's own requests in flight at once; one beyond this
/// is dropped, so a server that floods pings cannot pile up connections.
const SERVER_ANSWERS: usize = 8;
/// How long posting a notification or an answer to the server may take.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
/// What an event stream may begin with, to be skipped.
const BOM: &[u8] = b"\xef\xbb\xbf";

pub(super) struct HttpTransport {
    commands: UnboundedSender<Command>,
    connection: Arc<Connection>,
    /// Closes when the worker thread ends.
    finished: Mutex<mpsc::Receiver<()>>,
}

enum Command {
    Request {
        id: u64,
        message: Value,
        reply: mpsc::Sender<Reply>,
    },
    Notify {
        message: Value,
        done: mpsc::Sender<Result<(), RequestError>>,
    },
    /// Stops the request with this ID.
    Forget(u64),
    /// Ends the session and the worker.
    Shutdown,
}

struct Connection {
    client: reqwest::Client,
    url: Url,
    /// `scheme://host[:port]`, for messages.
    origin: String,
    headers: HeaderMap,
    /// From the initialize response, sent on every later request.
    session: Mutex<Option<HeaderValue>>,
    /// The negotiated revision, sent on every request after initialize.
    protocol_version: Mutex<Option<HeaderValue>>,
    /// Permits for answers to the server's requests.
    answers: Arc<tokio::sync::Semaphore>,
}

impl HttpTransport {
    pub(super) fn start(
        name: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        let not_started = |why: String| format!("MCP server {name} was not started: {why}.");
        let url =
            Url::parse(url).map_err(|err| not_started(format!("its url is invalid: {err}")))?;
        let mut header_map = HeaderMap::new();
        for (header, value) in headers {
            // Settings validated both; never repeat the value.
            let header = HeaderName::from_bytes(header.as_bytes())
                .map_err(|_| not_started(format!("'{header}' is not a header name")))?;
            let mut value = HeaderValue::from_str(value)
                .map_err(|_| not_started(format!("the value of header '{header}' is not valid")))?;
            value.set_sensitive(true);
            header_map.insert(header, value);
        }
        let mut client = reqwest::Client::builder()
            // A redirect would carry the headers to another address.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("socium/", env!("CARGO_PKG_VERSION")));
        // Plain http is allowed only to loopback, where a proxy from the
        // environment would see the headers in the clear.
        if is_loopback(&url) {
            client = client.no_proxy();
        }
        let client = client
            .build()
            .map_err(|err| not_started(format!("its HTTP client could not be built: {err}")))?;
        let connection = Arc::new(Connection {
            client,
            origin: url.origin().ascii_serialization(),
            url,
            headers: header_map,
            session: Mutex::new(None),
            protocol_version: Mutex::new(None),
            answers: Arc::new(tokio::sync::Semaphore::new(SERVER_ANSWERS)),
        });
        let (commands, receiver) = unbounded_channel();
        let (finished_sender, finished) = mpsc::channel();
        let worker = connection.clone();
        std::thread::Builder::new()
            .name(format!("mcp-{name}"))
            .spawn(move || {
                let _finished = finished_sender;
                run(&worker, receiver);
            })
            .map_err(|err| not_started(format!("its thread could not start: {err}")))?;
        Ok(Self {
            commands,
            connection,
            finished: Mutex::new(finished),
        })
    }

    pub(super) fn origin(&self) -> &str {
        &self.connection.origin
    }

    pub(super) fn send(
        &self,
        id: u64,
        message: &Value,
    ) -> Result<mpsc::Receiver<Reply>, RequestError> {
        let (reply, responses) = mpsc::channel();
        self.commands
            .send(Command::Request {
                id,
                message: message.clone(),
                reply,
            })
            .map_err(|_| RequestError::Gone)?;
        Ok(responses)
    }

    pub(super) fn forget(&self, id: u64) {
        let _ = self.commands.send(Command::Forget(id));
    }

    /// Posts a notification; with a deadline, waits for the server to
    /// accept it, so the next request cannot overtake it.
    pub(super) fn notify(
        &self,
        message: &Value,
        deadline: Option<Instant>,
    ) -> Result<(), RequestError> {
        let (done, accepted) = mpsc::channel();
        self.commands
            .send(Command::Notify {
                message: message.clone(),
                done,
            })
            .map_err(|_| RequestError::Gone)?;
        let Some(deadline) = deadline else {
            return Ok(());
        };
        match accepted.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(RequestError::TimedOut),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(RequestError::Gone),
        }
    }

    /// Sends `MCP-Protocol-Version: <version>` from now on; `version` is
    /// one of the revisions the client speaks.
    pub(super) fn negotiated(&self, version: &'static str) {
        *lock(&self.connection.protocol_version) = Some(HeaderValue::from_static(version));
    }
}

impl Drop for HttpTransport {
    /// Ends the session, waiting a short while at most. Only channels are
    /// touched here, so this is safe inside an async runtime.
    fn drop(&mut self) {
        if self.commands.send(Command::Shutdown).is_ok() {
            let _ =
                lock(&self.finished).recv_timeout(END_SESSION_TIMEOUT + Duration::from_millis(500));
        }
    }
}

fn run(connection: &Arc<Connection>, mut commands: UnboundedReceiver<Command>) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        // Callers see the closed channel as a server that is gone.
        return;
    };
    runtime.block_on(async {
        let mut requests: HashMap<u64, tokio::task::AbortHandle> = HashMap::new();
        while let Some(command) = commands.recv().await {
            requests.retain(|_, task| !task.is_finished());
            match command {
                Command::Request { id, message, reply } => {
                    let connection = connection.clone();
                    let task = tokio::spawn(async move {
                        let _ = reply.send(connection.request(id, &message).await);
                    });
                    requests.insert(id, task.abort_handle());
                }
                Command::Notify { message, done } => {
                    let connection = connection.clone();
                    tokio::spawn(async move {
                        let posted =
                            tokio::time::timeout(CONTROL_TIMEOUT, connection.post(&message))
                                .await
                                .unwrap_or(Err(RequestError::TimedOut));
                        let _ = done.send(posted);
                    });
                }
                Command::Forget(id) => {
                    if let Some(task) = requests.remove(&id) {
                        task.abort();
                    }
                }
                Command::Shutdown => break,
            }
        }
        let _ = tokio::time::timeout(END_SESSION_TIMEOUT, connection.end_session()).await;
    });
}

impl Connection {
    /// Sends `body` and checks the status. An error is a phrase that
    /// follows "it", and never holds a header value.
    async fn send(&self, method: Method, body: Option<&Value>) -> Result<Response, RequestError> {
        let session = lock(&self.session).clone();
        let mut request = self
            .client
            .request(method, self.url.clone())
            .headers(self.headers.clone())
            .header(ACCEPT, "application/json, text/event-stream");
        if let Some(body) = body {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(body).unwrap_or_default());
        }
        if let Some(session) = &session {
            request = request.header(SESSION_HEADER, session.clone());
        }
        if let Some(version) = lock(&self.protocol_version).clone() {
            request = request.header(VERSION_HEADER, version);
        }
        let response = request.send().await.map_err(|err| {
            RequestError::Transport(format!("could not be reached: {}", describe(&err)))
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        if status == StatusCode::NOT_FOUND && session.is_some() {
            // The next initialize starts a new session.
            lock(&self.session).take();
            lock(&self.protocol_version).take();
            return Err(RequestError::SessionEnded);
        }
        Err(RequestError::Transport(match status {
            StatusCode::UNAUTHORIZED => "needs authorization (HTTP 401); Socium sends only the headers of its [mcp.servers] table and does not support OAuth".to_string(),
            _ => format!("answered HTTP {status}{}", quoted_error(response).await),
        }))
    }

    /// Posts a notification or response, which the server accepts with
    /// 202 and no body.
    async fn post(&self, message: &Value) -> Result<(), RequestError> {
        self.send(Method::POST, Some(message)).await.map(drop)
    }

    /// Posts the request `id` and reads its response from the JSON body or
    /// the event stream.
    async fn request(self: &Arc<Self>, id: u64, message: &Value) -> Reply {
        let response = self.send(Method::POST, Some(message)).await?;
        if let Some(session) = response.headers().get(SESSION_HEADER) {
            let mut current = lock(&self.session);
            if current.is_none() {
                *current = Some(session.clone());
            }
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if content_type.starts_with("text/event-stream") {
            return self.read_events(response, id).await;
        }
        if !content_type.starts_with("application/json") {
            return Err(RequestError::Transport(format!(
                "answered with content type '{content_type}' instead of JSON or an event stream"
            )));
        }
        let body = read_body(response, MAX_MESSAGE_BYTES)
            .await
            .map_err(|why| Self::broke(&why))?;
        match serde_json::from_slice::<Value>(&body) {
            Ok(message @ Value::Object(_)) if is_response_to(&message, id) => Ok(message),
            _ => Err(RequestError::Transport(
                "answered with a body that is not the response".into(),
            )),
        }
    }

    /// The response to `id` from an event stream. The server's requests on
    /// the stream are answered (pings only, as over stdio) and its
    /// notifications skipped.
    async fn read_events(self: &Arc<Self>, mut response: Response, id: u64) -> Reply {
        let mut stream = EventStream::new(MAX_MESSAGE_BYTES);
        loop {
            let chunk = response
                .chunk()
                .await
                .map_err(|err| Self::broke(&describe(&err)))?;
            let Some(chunk) = chunk else {
                return Err(RequestError::Transport(
                    "closed its event stream before answering".into(),
                ));
            };
            for data in stream.push(&chunk).map_err(Self::broke)? {
                if let Some(found) = self.event(&data, id) {
                    return Ok(found);
                }
            }
        }
    }

    /// Handles one event's data: the response to `id`, or `None`.
    fn event(self: &Arc<Self>, data: &[u8], id: u64) -> Option<Value> {
        let Ok(Value::Object(message)) = serde_json::from_slice::<Value>(data) else {
            return None;
        };
        let message = Value::Object(message);
        match (message.get("method"), message.get("id")) {
            (None, Some(_)) if is_response_to(&message, id) => Some(message),
            (Some(method), Some(request)) => {
                let reply = if method == "ping" {
                    json!({"jsonrpc": "2.0", "id": request, "result": {}})
                } else {
                    json!({"jsonrpc": "2.0", "id": request, "error": {"code": -32601, "message": "Method not found"}})
                };
                if let Ok(permit) = self.answers.clone().try_acquire_owned() {
                    let connection = self.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let _ =
                            tokio::time::timeout(CONTROL_TIMEOUT, connection.post(&reply)).await;
                    });
                }
                None
            }
            _ => None,
        }
    }

    async fn end_session(&self) {
        if lock(&self.session).is_none() {
            return;
        }
        // The server may answer 405 when it does not let clients end
        // sessions; either way there is nothing more to do.
        let _ = self.send(Method::DELETE, None).await;
        lock(&self.session).take();
    }

    fn broke(why: &str) -> RequestError {
        RequestError::Transport(format!("broke off its answer: {why}"))
    }
}

fn is_response_to(message: &Value, id: u64) -> bool {
    message.get("method").is_none() && message["id"].as_u64() == Some(id)
}

/// Whether `url` names this machine, by name or by a loopback address.
fn is_loopback(url: &Url) -> bool {
    match url.host_str() {
        Some(host) if host.eq_ignore_ascii_case("localhost") => true,
        Some(host) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback()),
        None => false,
    }
}

/// The data of each event in an event stream, read as the WHATWG parser
/// does: lines end in CRLF, LF, or CR, even across chunks; a leading BOM is
/// skipped; and a line or event larger than `limit` is refused before it
/// is kept.
struct EventStream {
    /// The line not ended yet.
    line: Vec<u8>,
    data: Vec<u8>,
    limit: usize,
    /// Whether the start, which may hold a BOM, has been read.
    started: bool,
    /// Whether the last chunk ended in CR, whose LF may open the next one.
    after_cr: bool,
}

impl EventStream {
    fn new(limit: usize) -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            limit,
            started: false,
            after_cr: false,
        }
    }

    /// Reads `chunk` and returns the data of the events it ends.
    fn push(&mut self, mut chunk: &[u8]) -> Result<Vec<Vec<u8>>, &'static str> {
        if std::mem::take(&mut self.after_cr) && chunk.first() == Some(&b'\n') {
            chunk = &chunk[1..];
        }
        let mut events = Vec::new();
        while !chunk.is_empty() {
            let end = chunk.iter().position(|byte| matches!(byte, b'\n' | b'\r'));
            let part = &chunk[..end.unwrap_or(chunk.len())];
            if self.line.len() + part.len() > self.limit {
                return Err("an event is larger than the limit");
            }
            self.line.extend_from_slice(part);
            let Some(end) = end else { break };
            let mut next = end + 1;
            if chunk[end] == b'\r' {
                match chunk.get(next) {
                    Some(b'\n') => next += 1,
                    Some(_) => {}
                    None => self.after_cr = true,
                }
            }
            chunk = &chunk[next..];
            let line = std::mem::take(&mut self.line);
            if let Some(event) = self.line_ended(&line)? {
                events.push(event);
            }
        }
        Ok(events)
    }

    /// Takes in one line; an empty one ends the event.
    fn line_ended(&mut self, mut line: &[u8]) -> Result<Option<Vec<u8>>, &'static str> {
        if !self.started {
            self.started = true;
            line = line.strip_prefix(BOM).unwrap_or(line);
        }
        if line.is_empty() {
            if self.data.is_empty() {
                return Ok(None);
            }
            let mut data = std::mem::take(&mut self.data);
            data.pop();
            return Ok(Some(data));
        }
        // `event:`, `id:`, `retry:` and comments carry nothing used here.
        let value = match line.strip_prefix(b"data") {
            Some([]) => &[][..],
            Some([b':', value @ ..]) => value.strip_prefix(b" ").unwrap_or(value),
            _ => return Ok(None),
        };
        if self.data.len() + value.len() + 1 > self.limit {
            return Err("an event is larger than the limit");
        }
        // Each data line ends in LF; the last one is dropped at the end.
        self.data.extend_from_slice(value);
        self.data.push(b'\n');
        Ok(None)
    }
}

async fn read_body(mut response: Response, limit: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|err| describe(&err))? {
        if body.len() + chunk.len() > limit {
            return Err("the body is larger than the limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// `": <message>"` when an error body is a JSON-RPC error, else nothing.
async fn quoted_error(response: Response) -> String {
    let Ok(body) = read_body(response, 64 * 1024).await else {
        return String::new();
    };
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return String::new();
    };
    match body["error"]["message"].as_str() {
        Some(message) if !message.trim().is_empty() => {
            format!(
                ": {}",
                message.chars().take(QUOTED_ERROR_CHARS).collect::<String>()
            )
        }
        _ => String::new(),
    }
}

/// The error and its causes, which reqwest keeps apart.
fn describe(err: &reqwest::Error) -> String {
    let mut text = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(chunks: &[&[u8]], limit: usize) -> Result<Vec<String>, &'static str> {
        let mut stream = EventStream::new(limit);
        let mut events = Vec::new();
        for chunk in chunks {
            for data in stream.push(chunk)? {
                events.push(String::from_utf8(data).unwrap());
            }
        }
        Ok(events)
    }

    #[test]
    fn an_event_stream_ends_lines_in_crlf_lf_or_cr_even_across_chunks() {
        let stream: &[u8] =
            b"\xef\xbb\xbfdata: a\r\n\r\ndata: b\rdata:c\r\rid: 1\ndata\ndata: d\n\n: note\n";
        for split in 0..=stream.len() {
            let (first, second) = stream.split_at(split);
            assert_eq!(
                events(&[first, second], 1024),
                Ok(vec!["a".into(), "b\nc".into(), "\nd".into()]),
                "split at {split}"
            );
        }
    }

    #[test]
    fn an_event_larger_than_the_limit_is_refused_wherever_the_chunks_break() {
        let stream = format!("data: {}\n\n", "x".repeat(40));
        for split in 0..=stream.len() {
            let (first, second) = stream.as_bytes().split_at(split);
            assert!(events(&[first, second], 32).is_err(), "split at {split}");
        }
        assert_eq!(events(&[stream.as_bytes()], 64).unwrap().len(), 1);
    }

    #[test]
    fn only_loopback_urls_bypass_a_proxy() {
        for (url, loopback) in [
            ("http://127.0.0.1:8931/mcp", true),
            ("http://localhost/mcp", true),
            ("http://[::1]:80/mcp", true),
            ("https://mcp.example.com/mcp", false),
            ("https://10.0.0.1/mcp", false),
        ] {
            assert_eq!(is_loopback(&Url::parse(url).unwrap()), loopback, "{url}");
        }
    }
}
