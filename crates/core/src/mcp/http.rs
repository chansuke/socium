//! The Streamable HTTP transport (MCP 2025-06-18, ADR 30): every message
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
//! what the server answers, before it cuts them short. So is the OAuth
//! token of a server the user logged in to (ADR 31), which is sent only
//! when no `Authorization` header is configured.

use super::oauth::OAuthSession;
use super::{MAX_MESSAGE_BYTES, Reply, RequestError, Secrets, lock};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Response, StatusCode, Url};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
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
/// What Socium quotes from an HTTP error body or header is cut here, after
/// secrets are blanked out of it.
const QUOTED_ERROR_CHARS: usize = 300;
/// How long a token renewal under way may take when the server is
/// dropped: as long as its request to the token endpoint may.
const RENEWAL_SETTLE_TIMEOUT: Duration = super::oauth::REQUEST_TIMEOUT;
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

/// How requests to a server are authorized.
pub(super) enum Authorization {
    /// No `Authorization` header and no stored token: a 401 means the user
    /// has to log in.
    None,
    /// The configured headers hold `Authorization`, which wins over OAuth.
    Header,
    OAuth(Arc<OAuthSession>),
}

struct Connection {
    clients: HttpClients,
    /// The one of `clients` for `url`.
    client: reqwest::Client,
    url: Url,
    /// `scheme://host[:port]`, for messages.
    origin: String,
    headers: HeaderMap,
    /// The values of `headers`, blanked out of what Socium quotes.
    secrets: Secrets,
    auth: Authorization,
    session: Mutex<Session>,
    /// Permits for answers to the server's requests.
    answers: Arc<tokio::sync::Semaphore>,
    /// Set once the server refused Socium for want of a login.
    login_needed: std::sync::atomic::AtomicBool,
}

/// The session the server started, as Socium knows it.
#[derive(Default)]
struct Session {
    /// From the initialize response, sent on every later request.
    id: Option<HeaderValue>,
    /// The negotiated revision, sent on every request after initialize.
    version: Option<HeaderValue>,
    /// Counts the sessions the server ended. A 404 ends the session only
    /// when the request that got it was sent in the current one, so a late
    /// answer to a request of an ended session cannot end its successor.
    generation: u64,
    /// Whether the server ended the session and no initialize has started
    /// another since.
    ended: bool,
}

/// The clients for an MCP server and its authorization server. They follow
/// no redirect, since one would carry a token or a code to another address.
/// They differ only in the proxy: plain http is allowed only to loopback,
/// where a proxy from the environment would see a token in the clear.
#[derive(Clone)]
pub(super) struct HttpClients {
    direct: reqwest::Client,
    proxied: reqwest::Client,
    /// How names are resolved; `None` is the system's resolver.
    lookup: Option<Lookup>,
    /// Whether URLs that discovery found may point at this machine or a
    /// private network: only when the configured server is there too.
    allow_internal: bool,
}

/// Resolves a host and port, as tests script it.
type Lookup = Arc<dyn Fn(&str, u16) -> std::io::Result<Vec<SocketAddr>> + Send + Sync>;

#[expect(
    clippy::disallowed_methods,
    reason = "MCP clients add their own resolver and proxy choice; this sets the same no-redirect policy and connect timeout as crate::http_client"
)]
fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("socium/", env!("CARGO_PKG_VERSION")))
}

impl HttpClients {
    pub(super) fn new() -> reqwest::Result<Self> {
        Ok(Self {
            direct: client_builder().no_proxy().build()?,
            proxied: client_builder().build()?,
            lookup: None,
            allow_internal: false,
        })
    }

    /// These clients for the MCP server at `server`.
    pub(super) fn serving(mut self, server: &Url) -> Self {
        self.allow_internal = is_internal(server);
        self
    }

    #[cfg(test)]
    pub(super) fn with_lookup(lookup: Lookup) -> Self {
        Self {
            lookup: Some(lookup),
            ..Self::new().unwrap()
        }
    }

    /// As [`Self::with_lookup`], with every request that may use a proxy
    /// going to `proxy`, as one from the environment would.
    #[cfg(test)]
    pub(super) fn with_proxy(lookup: Lookup, proxy: &str) -> Self {
        Self {
            proxied: client_builder()
                .proxy(reqwest::Proxy::all(proxy).unwrap())
                .build()
                .unwrap(),
            ..Self::with_lookup(lookup)
        }
    }

    /// Checks a URL that discovery found rather than the user. Unless the
    /// configured server is on this machine or a private network itself,
    /// its host must name no such address: a name is resolved once and
    /// refused if any answer is internal. Returns the host and those
    /// answers, to pin the request to; `None` when there is nothing to pin.
    pub(super) async fn check_discovered(
        &self,
        url: &Url,
    ) -> Result<Option<(String, Vec<SocketAddr>)>, String> {
        if self.allow_internal {
            return Ok(None);
        }
        if is_internal(url) {
            return Err(format!(
                "{url} points at this machine or a private network, and the server is not there"
            ));
        }
        let Some(host) = url.host_str() else {
            return Err(format!("{url} has no host"));
        };
        if host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok()
        {
            // An address, checked above.
            return Ok(None);
        }
        let port = url.port_or_known_default().unwrap_or(443);
        let addresses = match &self.lookup {
            Some(lookup) => lookup(host, port),
            None => tokio::net::lookup_host((host, port))
                .await
                .map(|found| found.collect()),
        }
        .map_err(|err| format!("{host} could not be resolved: {err}"))?;
        if addresses.is_empty() {
            return Err(format!("{host} resolves to no address"));
        }
        if let Some(inside) = addresses
            .iter()
            .find(|address| is_internal_address(address.ip()))
        {
            return Err(format!(
                "{host} resolves to {}, an address on this machine or a private network, and the server is not there",
                inside.ip()
            ));
        }
        Ok(Some((host.to_string(), addresses)))
    }

    /// The client for a URL that discovery found, checked as
    /// [`Self::check_discovered`] does. Its requests go to the addresses
    /// checked, so a second lookup cannot turn the name into an internal
    /// one, and never through a proxy from the environment, which would
    /// resolve the name itself (RFC 9728 7.7).
    pub(super) async fn for_discovered(&self, url: &Url) -> Result<reqwest::Client, String> {
        if self.allow_internal {
            return Ok(self.to(url).clone());
        }
        match self.check_discovered(url).await? {
            None => Ok(self.direct.clone()),
            Some((host, addresses)) => client_builder()
                .no_proxy()
                .resolve_to_addrs(&host, &addresses)
                .build()
                .map_err(|err| format!("its HTTP client could not be built: {err}")),
        }
    }

    /// The client for requests to `url`.
    pub(super) fn to(&self, url: &Url) -> &reqwest::Client {
        if is_loopback(url) {
            &self.direct
        } else {
            &self.proxied
        }
    }
}

impl HttpTransport {
    pub(super) fn start(
        name: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        auth: Authorization,
    ) -> Result<Self, String> {
        // A sentence that says why it was not started.
        let not_started = |why: String| format!("{why}.");
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
        let clients = HttpClients::new()
            .map_err(|err| not_started(format!("its HTTP client could not be built: {err}")))?
            .serving(&url);
        let connection = Arc::new(Connection {
            client: clients.to(&url).clone(),
            clients,
            origin: url.origin().ascii_serialization(),
            url,
            headers: header_map,
            secrets: Secrets::of_values(headers.values()),
            auth,
            session: Mutex::default(),
            answers: Arc::new(tokio::sync::Semaphore::new(SERVER_ANSWERS)),
            login_needed: std::sync::atomic::AtomicBool::new(false),
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

    /// Whether the server has refused Socium for want of a login since it
    /// started.
    pub(super) fn needs_login(&self) -> bool {
        self.connection
            .login_needed
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Sends `MCP-Protocol-Version: <version>` from now on; `version` is
    /// one of the revisions the client speaks. The session initialize
    /// started is the current one.
    pub(super) fn negotiated(&self, version: &'static str) {
        let mut session = lock(&self.connection.session);
        session.version = Some(HeaderValue::from_static(version));
        session.ended = false;
    }

    /// Whether the server ended the session (HTTP 404) and none has been
    /// started since.
    pub(super) fn session_ended(&self) -> bool {
        lock(&self.connection.session).ended
    }

    /// `text` without the configured header values and the OAuth tokens
    /// sent to the server.
    pub(super) fn redact(&self, text: &str) -> String {
        self.connection.redact(text)
    }
}

impl Drop for HttpTransport {
    /// Ends the session, waiting a short while at most, or as long as a
    /// token renewal under way may take, so a renewed token is saved. Only
    /// channels are touched here, so this is safe inside an async runtime.
    fn drop(&mut self) {
        let renewing = match &self.connection.auth {
            Authorization::OAuth(session) => session.renewing(),
            Authorization::None | Authorization::Header => false,
        };
        let mut wait = END_SESSION_TIMEOUT + Duration::from_millis(500);
        if renewing {
            wait += RENEWAL_SETTLE_TIMEOUT;
        }
        if self.commands.send(Command::Shutdown).is_ok() {
            let _ = lock(&self.finished).recv_timeout(wait);
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
        // A renewal runs on this runtime: one the authorization server has
        // answered would lose its token, whose refresh token it used up.
        if let Authorization::OAuth(session) = &connection.auth {
            session.settle(RENEWAL_SETTLE_TIMEOUT).await;
        }
    });
}

impl Connection {
    /// Sends `body` and checks the status; with OAuth, renews the token and
    /// tries once more after a 401. An error is a phrase that follows "it",
    /// and never holds a header value.
    async fn send(&self, method: Method, body: Option<&Value>) -> Result<Response, RequestError> {
        let sent = self.exchange(method, body).await;
        if let Err(RequestError::NeedsLogin) = &sent {
            self.login_needed
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        sent
    }

    async fn exchange(
        &self,
        method: Method,
        body: Option<&Value>,
    ) -> Result<Response, RequestError> {
        let mut renewed = false;
        loop {
            let bearer = match &self.auth {
                Authorization::OAuth(session) => Some(
                    session
                        .access_token(&self.clients)
                        .await
                        .map_err(|()| RequestError::NeedsLogin)?,
                ),
                Authorization::None | Authorization::Header => None,
            };
            let (session, version, generation) = {
                let current = lock(&self.session);
                (
                    current.id.clone(),
                    current.version.clone(),
                    current.generation,
                )
            };
            let mut request = self
                .client
                .request(method.clone(), self.url.clone())
                .headers(self.headers.clone())
                .header(ACCEPT, "application/json, text/event-stream");
            if let Some(bearer) = &bearer {
                // The token endpoint's answer was checked to fit.
                let mut value = HeaderValue::from_str(&format!("Bearer {bearer}"))
                    .map_err(|_| RequestError::NeedsLogin)?;
                value.set_sensitive(true);
                request = request.header(AUTHORIZATION, value);
            }
            if let Some(body) = body {
                request = request
                    .header(CONTENT_TYPE, "application/json")
                    .body(serde_json::to_vec(body).unwrap_or_default());
            }
            if let Some(session) = &session {
                request = request.header(SESSION_HEADER, session.clone());
            }
            if let Some(version) = version {
                request = request.header(VERSION_HEADER, version);
            }
            let response = request.send().await.map_err(|err| {
                RequestError::Transport(format!("could not be reached: {}", describe(&err)))
            })?;
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            if status == StatusCode::UNAUTHORIZED {
                match (&self.auth, &bearer) {
                    (Authorization::OAuth(session), Some(sent)) if !renewed => {
                        session
                            .refused(&self.clients, sent)
                            .await
                            .map_err(|()| RequestError::NeedsLogin)?;
                        renewed = true;
                        continue;
                    }
                    (Authorization::Header, _) => {
                        return Err(RequestError::Transport(
                            "refused the Authorization header of its [mcp.servers] table (HTTP 401)"
                                .into(),
                        ));
                    }
                    _ => return Err(RequestError::NeedsLogin),
                }
            }
            if status == StatusCode::NOT_FOUND && session.is_some() {
                // The next initialize starts a new session.
                let mut current = lock(&self.session);
                if current.generation == generation {
                    *current = Session {
                        generation: generation + 1,
                        ended: true,
                        ..Session::default()
                    };
                }
                return Err(RequestError::SessionEnded);
            }
            // The server may write what it was sent into its error.
            let quoted = match error_message(response).await {
                Some(message) => format!(": {}", self.quote(&message)),
                None => String::new(),
            };
            return Err(RequestError::Transport(format!(
                "answered HTTP {status}{quoted}"
            )));
        }
    }

    /// Posts a notification or response, which the server accepts with
    /// 202 and no body.
    async fn post(&self, message: &Value) -> Result<(), RequestError> {
        let status = self.send(Method::POST, Some(message)).await?.status();
        if status != StatusCode::ACCEPTED {
            return Err(RequestError::Transport(format!(
                "answered HTTP {status} instead of 202 Accepted to a notification"
            )));
        }
        Ok(())
    }

    /// `text` without the configured header values and the OAuth tokens
    /// sent to the server.
    fn redact(&self, text: &str) -> String {
        let text = self.secrets.redact(text);
        match &self.auth {
            Authorization::OAuth(session) => session.redact(&text),
            Authorization::None | Authorization::Header => text,
        }
    }

    /// `text` from the server, blanked out first and then cut short, so no
    /// part of a secret is left at the cut.
    fn quote(&self, text: &str) -> String {
        self.redact(text).chars().take(QUOTED_ERROR_CHARS).collect()
    }

    /// Posts the request `id` and reads its response from the JSON body or
    /// the event stream.
    async fn request(self: &Arc<Self>, id: u64, message: &Value) -> Reply {
        let response = self.send(Method::POST, Some(message)).await?;
        if let Some(session) = response.headers().get(SESSION_HEADER) {
            let mut current = lock(&self.session);
            if current.id.is_none() {
                current.id = Some(session.clone());
            }
        }
        match media_type(response.headers()).as_str() {
            "text/event-stream" => return self.read_events(response, id).await,
            "application/json" => {}
            other => {
                return Err(RequestError::Transport(format!(
                    "answered with content type '{}' instead of JSON or an event stream",
                    self.quote(other)
                )));
            }
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
            // The events the chunk completed come first: a response among
            // them is the answer, whatever follows it in the same chunk.
            let (events, read) = stream.push(&chunk);
            for data in events {
                if let Some(found) = self.event(&data, id) {
                    return Ok(found);
                }
            }
            read.map_err(Self::broke)?;
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
        if lock(&self.session).id.is_none() {
            return;
        }
        // The server may answer 405 when it does not let clients end
        // sessions; either way there is nothing more to do.
        let _ = self.send(Method::DELETE, None).await;
        lock(&self.session).id.take();
    }

    fn broke(why: &str) -> RequestError {
        RequestError::Transport(format!("broke off its answer: {why}"))
    }
}

/// The media type of a response's `Content-Type`, without parameters and
/// in lower case (RFC 9110 8.3.1).
fn media_type(headers: &HeaderMap) -> String {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn is_response_to(message: &Value, id: u64) -> bool {
    message.get("method").is_none() && message["id"].as_u64() == Some(id)
}

/// Whether `url` names this machine or a private network by its host:
/// `localhost` or an address [`is_internal_address`] refuses.
pub(super) fn is_internal(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    match host.parse::<std::net::IpAddr>() {
        Ok(address) => is_internal_address(address),
        Err(_) => host == "localhost" || host.ends_with(".localhost"),
    }
}

/// A loopback, private, link-local, shared, broadcast, or unspecified
/// address, IPv4 mapped into IPv6 included.
pub(super) fn is_internal_address(address: std::net::IpAddr) -> bool {
    let v4 = |address: std::net::Ipv4Addr| {
        address.is_loopback()
            || address.is_private()
            || address.is_link_local()
            || address.is_unspecified()
            || address.is_broadcast()
            // 100.64.0.0/10, shared address space.
            || (address.octets()[0] == 100 && address.octets()[1] & 0xc0 == 64)
    };
    match address {
        std::net::IpAddr::V4(address) => v4(address),
        std::net::IpAddr::V6(address) => {
            address.is_loopback()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local()
                || address.to_ipv4_mapped().is_some_and(v4)
        }
    }
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

    /// Reads `chunk` and returns the data of the events it ends, and
    /// whether it could read all of it: the events ended before a part
    /// that is too large are returned too.
    fn push(&mut self, mut chunk: &[u8]) -> (Vec<Vec<u8>>, Result<(), &'static str>) {
        if std::mem::take(&mut self.after_cr) && chunk.first() == Some(&b'\n') {
            chunk = &chunk[1..];
        }
        let mut events = Vec::new();
        while !chunk.is_empty() {
            let end = chunk.iter().position(|byte| matches!(byte, b'\n' | b'\r'));
            let part = &chunk[..end.unwrap_or(chunk.len())];
            if self.line.len() + part.len() > self.limit {
                return (events, Err("an event is larger than the limit"));
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
            match self.line_ended(&line) {
                Ok(Some(event)) => events.push(event),
                Ok(None) => {}
                Err(why) => return (events, Err(why)),
            }
        }
        (events, Ok(()))
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

pub(super) async fn read_body(mut response: Response, limit: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|err| describe(&err))? {
        if body.len() + chunk.len() > limit {
            return Err("the body is larger than the limit".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The message of a JSON-RPC error in an HTTP error body, whole: it is
/// blanked out before it is cut.
async fn error_message(response: Response) -> Option<String> {
    let body = read_body(response, 64 * 1024).await.ok()?;
    let body = serde_json::from_slice::<Value>(&body).ok()?;
    body["error"]["message"]
        .as_str()
        .filter(|message| !message.trim().is_empty())
        .map(str::to_string)
}

/// The error and its causes, which reqwest keeps apart.
pub(super) fn describe(err: &reqwest::Error) -> String {
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
            let (ended, read) = stream.push(chunk);
            read?;
            for data in ended {
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
    fn a_response_an_event_stream_completed_is_kept_whatever_follows_it() {
        // The response, then a line too long to keep.
        let stream = format!("data: {{\"id\": 1}}\n\ndata: {}\n", "x".repeat(64));
        let response = b"{\"id\": 1}".to_vec();
        let ends = stream.find("\n\n").unwrap() + 2;
        for split in [stream.len(), ends, ends + 3] {
            let (first, second) = stream.as_bytes().split_at(split);
            let mut events = EventStream::new(32);
            let (mut read, mut result) = events.push(first);
            if result.is_ok() {
                let (more, last) = events.push(second);
                read.extend(more);
                result = last;
            }
            assert_eq!(read, std::slice::from_ref(&response), "split at {split}");
            assert!(result.is_err(), "split at {split}");
        }
    }

    #[test]
    fn only_json_and_event_streams_are_read_whatever_their_parameters_and_case() {
        let of = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_str(value).unwrap());
            media_type(&headers)
        };
        assert_eq!(of("application/json"), "application/json");
        assert_eq!(of("Application/JSON ; charset=utf-8"), "application/json");
        assert_eq!(of("text/event-stream;charset=UTF-8"), "text/event-stream");
        for other in [
            "application/json-patch+json",
            "application/jsonx",
            "text/event-stream-extra",
            "text/plain",
        ] {
            assert_ne!(of(other), "application/json", "{other}");
            assert_ne!(of(other), "text/event-stream", "{other}");
        }
        assert_eq!(media_type(&HeaderMap::new()), "");
    }

    #[test]
    fn a_discovered_url_never_goes_through_a_proxy() {
        let (proxy_port, proxied) = internal_service();
        // TEST-NET-1, which goes nowhere.
        let clients = HttpClients::with_proxy(
            std::sync::Arc::new(|_: &str, port| {
                Ok(vec![std::net::SocketAddr::from(([192, 0, 2, 1], port))])
            }),
            &format!("http://127.0.0.1:{proxy_port}"),
        );
        let get = |client: reqwest::Client, url: Url| async move {
            client
                .get(url)
                .timeout(Duration::from_millis(500))
                .send()
                .await
        };

        for url in ["https://auth.example/token", "https://192.0.2.1/token"] {
            let url = Url::parse(url).unwrap();
            let client = run(clients.for_discovered(&url)).unwrap();
            let sent = run(get(client, url.clone()));
            assert!(sent.is_err(), "{url}");
        }
        assert_eq!(proxied.load(std::sync::atomic::Ordering::SeqCst), 0);

        // The configured server is reached through the proxy, as before.
        let url = Url::parse("https://mcp.example/mcp").unwrap();
        let _ = run(get(clients.to(&url).clone(), url));
        assert!(proxied.load(std::sync::atomic::Ordering::SeqCst) > 0);
    }

    /// A listener on loopback that counts the connections it gets.
    fn internal_service() -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let reached = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = reached.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                drop(stream);
            }
        });
        (port, reached)
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn a_discovered_name_that_resolves_to_this_machine_is_refused() {
        let (port, reached) = internal_service();
        let clients = HttpClients::with_lookup(std::sync::Arc::new(move |_: &str, _| {
            Ok(vec![std::net::SocketAddr::from(([127, 0, 0, 1], port))])
        }));
        let url = Url::parse(&format!("http://auth.example:{port}/token")).unwrap();

        let err = run(clients.for_discovered(&url)).err().unwrap();

        assert!(err.contains("auth.example"), "{err}");
        assert!(err.contains("127.0.0.1"), "{err}");
        assert_eq!(reached.load(std::sync::atomic::Ordering::SeqCst), 0);
        // A server that is itself on this machine may name such places.
        let local = clients.serving(&Url::parse("http://127.0.0.1:8/mcp").unwrap());
        assert!(run(local.for_discovered(&url)).is_ok());
    }

    #[test]
    fn a_second_lookup_cannot_turn_a_checked_name_into_this_machine() {
        let (port, reached) = internal_service();
        let lookups = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = lookups.clone();
        // First a public address (TEST-NET-1, which goes nowhere), then
        // loopback.
        let clients = HttpClients::with_lookup(std::sync::Arc::new(move |_: &str, _| {
            let first = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            let address = if first {
                [192, 0, 2, 1]
            } else {
                [127, 0, 0, 1]
            };
            Ok(vec![std::net::SocketAddr::from((address, port))])
        }));
        let url = Url::parse(&format!("http://auth.example:{port}/token")).unwrap();

        let sent = run(async {
            let client = clients.for_discovered(&url).await.unwrap();
            client
                .get(url.clone())
                .timeout(Duration::from_millis(500))
                .send()
                .await
        });

        assert!(sent.is_err());
        assert_eq!(reached.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(lookups.load(std::sync::atomic::Ordering::SeqCst), 1);
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
