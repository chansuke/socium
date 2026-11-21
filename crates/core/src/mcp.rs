//! A client for MCP servers the user configured in `config.toml` (ADR 29,
//! ADR 30, ADR 33): a server with `command` runs as a child process that
//! speaks newline-delimited JSON-RPC on stdin and stdout, on the host or,
//! with `sandbox = "read-only"`, in Socium's sandbox, and one with `url` is
//! reached over Streamable HTTP ([`http`]). Their tools are offered to the
//! main agent as `mcp__<server>__<tool>`.
//!
//! Only tools are used. A child on the host starts in `/`, finds its
//! program on a PATH without the workspace in it, and gets a fixed
//! environment (PATH, HOME, USER, LANG and what its table sets), so the
//! workspace cannot choose the code that runs and provider keys never reach
//! it. A sandboxed child starts in the workspace root, which it sees
//! read-only with the sensitive files hidden, without network, a home
//! directory, or the `[sandbox]` settings.

mod http;
pub(crate) mod oauth;
pub(crate) mod token_store;

use crate::TaskControl;
use crate::conversation::ToolDefinition;
use crate::execution::{ExecutionPolicy, SandboxExecutor, WorkspaceAccess};
use crate::settings::{McpDisabled, McpServerTransport};
use crate::settings::{McpSandboxSettings, McpServerSettings, McpTransportSettings, PathPatterns};
use crate::tool_error::ToolError;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};
use token_store::TokenStore;

/// The revision this client asks for.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// Revisions a server may answer with. Their tools methods are the same;
/// 2025-03-26 and 2024-11-05 also allow batches, which the reader accepts.
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// The longest a server may take to start and list its tools.
pub(crate) const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Host variables a server gets when they are set; everything else in
/// Socium's environment stays out.
const INHERITED_VARIABLES: &[&str] = &["PATH", "HOME", "USER", "LANG"];
/// What exposed tool names begin with.
const PREFIX: &str = crate::constants::MCP_TOOL_PREFIX;
/// The longest tool name model providers accept.
pub(crate) const MAX_TOOL_NAME: usize = 64;
/// Longer tool descriptions are cut, and a tool whose input schema is
/// larger is left out, so one server cannot fill every request.
const MAX_DESCRIPTION_CHARS: usize = 2048;
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
/// How many tools one server, and all servers together, may offer, and how
/// many bytes of descriptions and input schemas: providers refuse requests
/// with too many tools (OpenAI at 128), and every request carries them all.
const MAX_TOOLS_PER_SERVER: usize = 64;
const MAX_MCP_TOOLS: usize = 100;
const MAX_SERVER_DEFINITION_BYTES: usize = 128 * 1024;
const MAX_MCP_DEFINITION_BYTES: usize = 256 * 1024;
/// Lines of a server's standard error kept for warnings, and the longest
/// kept line.
const STDERR_LINES: usize = 20;
const STDERR_LINE_CHARS: usize = 400;
/// A message larger than this ends the connection.
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// Pages of `tools/list` read before giving up on a server that never
/// stops paginating.
const MAX_TOOL_PAGES: usize = 100;
/// Messages waiting for a child to read its input. A server with this many
/// unread is not reading, and further messages fail at once.
const OUTBOX_MESSAGES: usize = 64;
/// How long a stopping child gets after its input closes, and again after
/// SIGTERM, before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(1);
/// Values shorter than this are not treated as secrets, so a value such as
/// `1` does not blank out every digit of a warning.
const MIN_SECRET_CHARS: usize = 8;
/// How often a waiting call checks for cancellation.
const POLL: Duration = Duration::from_millis(50);

/// Warnings that arise after startup, such as a renewed token that could
/// not be saved, until the next turn shows them.
pub(crate) type Notices = Arc<Mutex<Vec<String>>>;

/// Whether `name` has the form of an MCP tool's exposed name.
pub(crate) fn is_mcp_tool(name: &str) -> bool {
    name.starts_with(PREFIX)
}

/// What a waiting request receives: the JSON-RPC response with its ID.
type Reply = Result<Value, RequestError>;
type Waiters = Arc<Mutex<Option<HashMap<u64, mpsc::Sender<Reply>>>>>;
/// Lines for a child's writer thread; `None` once its input is closed.
type Outbox = Arc<Mutex<Option<mpsc::SyncSender<Vec<u8>>>>>;

/// One connected server: the handshake, tool listing, calls and result
/// conversion are the same for every transport; only sending and
/// receiving differ.
pub(crate) struct McpServer {
    name: String,
    /// Runs in the read-only sandbox, so its tools run without asking.
    sandboxed: bool,
    transport: Transport,
    next_id: AtomicU64,
    secrets: Secrets,
    call_timeout: Duration,
    /// Held while a remote session that the server ended is started again,
    /// so concurrent calls start one, not one each.
    restarting: Mutex<()>,
}

enum Transport {
    Stdio(StdioTransport),
    Http(http::HttpTransport),
}

impl Transport {
    /// Sends the request `message` with ID `id`; its response arrives on
    /// the returned channel.
    fn send(&self, id: u64, message: &Value) -> Result<mpsc::Receiver<Reply>, RequestError> {
        match self {
            Self::Stdio(stdio) => stdio.send(id, message),
            Self::Http(http) => http.send(id, message),
        }
    }

    /// Stops waiting for the response to `id`.
    fn forget(&self, id: u64) {
        match self {
            Self::Stdio(stdio) => stdio.forget(id),
            Self::Http(http) => http.forget(id),
        }
    }

    /// Sends a notification. With a deadline, waits until the server took
    /// it, where the transport can tell.
    fn notify(&self, message: &Value, deadline: Option<Instant>) -> Result<(), RequestError> {
        match self {
            Self::Stdio(stdio) => stdio.queue(message),
            Self::Http(http) => http.notify(message, deadline),
        }
    }

    /// Where a remote server is, as `scheme://host[:port]`; `None` for a
    /// child process.
    fn origin(&self) -> Option<&str> {
        match self {
            Self::Stdio(_) => None,
            Self::Http(http) => Some(http.origin()),
        }
    }

    /// `" (exit status: 3)"` when a child has exited, else nothing.
    fn exit_status(&self) -> String {
        match self {
            Self::Stdio(stdio) => stdio.exit_status(),
            Self::Http(_) => String::new(),
        }
    }

    /// The last lines a child wrote to standard error, as a paragraph to
    /// append to a warning, or nothing.
    fn stderr_tail(&self) -> String {
        match self {
            Self::Stdio(stdio) => stdio.stderr_tail(),
            Self::Http(_) => String::new(),
        }
    }

    /// `text` without the OAuth tokens sent to a remote server.
    fn redact(&self, text: &str) -> String {
        match self {
            Self::Stdio(_) => text.to_string(),
            Self::Http(http) => http.redact(text),
        }
    }

    /// Starts stopping: a child's input closes, which tells it to exit. A
    /// remote session ends when its transport is dropped.
    fn close(&self) {
        match self {
            Self::Stdio(stdio) => stdio.close_input(),
            Self::Http(_) => {}
        }
    }
}

#[derive(Debug)]
enum RequestError {
    /// The server exited, closed its output, or stopped reading its input.
    Gone,
    Cancelled,
    TimedOut,
    /// A JSON-RPC error response.
    Refused(String),
    /// A response that is neither a result nor an error.
    Malformed,
    /// The HTTP exchange failed, as a phrase that follows "it".
    Transport(String),
    /// A remote server answered 404 to the session: it no longer knows it,
    /// so the request was not run.
    SessionEnded,
    /// A remote server refused Socium, which has no token or could not
    /// renew it: the user has to run `socium mcp login`.
    NeedsLogin,
}

/// When a request stops waiting for its response.
struct Wait<'a> {
    deadline: Instant,
    control: &'a TaskControl,
}

impl Wait<'_> {
    fn stopped(&self) -> Option<RequestError> {
        if self.control.is_cancelled() {
            Some(RequestError::Cancelled)
        } else if Instant::now() >= self.deadline {
            Some(RequestError::TimedOut)
        } else {
            None
        }
    }
}

/// Values a server's table sets, which a warning or error must not repeat
/// even when the server writes them back.
#[derive(Clone, Default)]
struct Secrets(Vec<String>);

impl Secrets {
    fn of(settings: &McpServerSettings) -> Self {
        let given = match &settings.transport {
            McpTransportSettings::Stdio { env, .. } => env,
            McpTransportSettings::Http { headers, .. } => headers,
        };
        Self::of_values(given.values())
    }

    fn of_values<'a>(given: impl Iterator<Item = &'a String>) -> Self {
        // A header such as `Bearer <token>` is also caught when the server
        // repeats the token alone.
        let mut values = given
            .flat_map(|value| [Some(value.as_str()), value.split_whitespace().last()])
            .flatten()
            .filter(|value| value.chars().count() >= MIN_SECRET_CHARS)
            .map(str::to_string)
            .collect::<Vec<_>>();
        // Longest first, so a value that contains another is replaced whole.
        values.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        values.dedup();
        Self(values)
    }

    fn redact(&self, text: &str) -> String {
        self.0.iter().fold(text.to_string(), |text, value| {
            text.replace(value, "[redacted]")
        })
    }

    /// `text`, which a reader cut short, without the start of a value it
    /// may end with: the rest of that value was never read, so it cannot be
    /// replaced.
    fn without_partial_end<'t>(&self, text: &'t str) -> &'t str {
        let partial = self
            .0
            .iter()
            .flat_map(|value| value.char_indices().skip(1).map(|(end, _)| &value[..end]))
            .filter(|start| text.ends_with(start))
            .map(str::len)
            .max()
            .unwrap_or(0);
        &text[..text.len() - partial]
    }

    /// The most bytes a value takes.
    fn longest(&self) -> usize {
        self.0.iter().map(String::len).max().unwrap_or(0)
    }
}

/// A tool as the server listed it.
struct RemoteTool {
    name: String,
    description: String,
    input_schema: Value,
}

/// What every server starts with: the same for all servers of one
/// [`McpRegistry::connect`].
#[derive(Clone, Copy)]
struct Startup<'a> {
    workspace_root: &'a Path,
    sensitive: &'a PathPatterns,
    host_env: &'a (dyn Fn(&str) -> Option<String> + Sync),
    tokens: Option<&'a TokenStore>,
    timeout: Duration,
    stop: &'a TaskControl,
    notices: &'a Notices,
}

impl McpServer {
    /// Connects to the server, performs the handshake, and lists its tools,
    /// within `startup.timeout` and until `startup.stop` is cancelled.
    fn connect(
        settings: &McpServerSettings,
        startup: Startup<'_>,
    ) -> Result<(Self, Vec<RemoteTool>), StartError> {
        let Startup {
            workspace_root,
            sensitive,
            host_env,
            tokens,
            timeout,
            stop,
            notices,
        } = startup;
        let secrets = Secrets::of(settings);
        let not_started = |reason: String| StartError::NotStarted {
            origin: None,
            reason: secrets.redact(&reason),
        };
        let wait = Wait {
            deadline: Instant::now() + timeout,
            control: stop,
        };
        let transport = match &settings.transport {
            McpTransportSettings::Stdio {
                command,
                env,
                sandbox,
            } => {
                let child = match sandbox {
                    None => Ok(host_command(command, env, workspace_root, host_env)),
                    Some(sandbox) => sandboxed_command(
                        command,
                        env,
                        sandbox,
                        workspace_root,
                        sensitive,
                        host_env,
                        &wait,
                    ),
                };
                // Listing the sensitive files can take a while.
                let child = child.and_then(|child| match stop.is_cancelled() {
                    true => Err("its start was cancelled before it ran.".to_string()),
                    false => Ok(child),
                });
                Transport::Stdio(
                    child
                        .and_then(|child| {
                            StdioTransport::spawn(child, &command[0], secrets.clone())
                        })
                        .map_err(not_started)?,
                )
            }
            McpTransportSettings::Http {
                url,
                headers,
                oauth,
            } => {
                let auth = if headers
                    .keys()
                    .any(|header| header.eq_ignore_ascii_case("authorization"))
                {
                    http::Authorization::Header
                } else {
                    // An unreadable token file counts as none: the server's
                    // 401 then asks the user to log in again. So does a
                    // token of another client or authorization server than
                    // the configured one, so the configured secret never
                    // goes to the token endpoint of an earlier login.
                    let stored = tokens.and_then(|store| {
                        let token = store.load(&settings.name, url).ok().flatten()?;
                        oauth::usable_with(&token, oauth.as_ref()).then(|| (store.clone(), token))
                    });
                    match stored {
                        Some((store, token)) => {
                            http::Authorization::OAuth(Arc::new(oauth::OAuthSession::new(
                                &settings.name,
                                store,
                                oauth.as_ref(),
                                token,
                                notices.clone(),
                            )))
                        }
                        None => http::Authorization::None,
                    }
                };
                Transport::Http(
                    http::HttpTransport::start(&settings.name, url, headers, auth)
                        .map_err(not_started)?,
                )
            }
        };
        let server = Self {
            name: settings.name.clone(),
            sandboxed: settings.sandbox().is_some(),
            transport,
            next_id: AtomicU64::new(1),
            secrets,
            call_timeout: settings.tool_timeout,
            restarting: Mutex::new(()),
        };
        let failed =
            |server: &Self, what: &str, err: RequestError| server.not_started(what, err, timeout);
        let initialized = server.handshake(&wait, timeout)?;
        let mut tools = Vec::new();
        // A server that does not offer tools has none to list.
        if !initialized["capabilities"]["tools"].is_object() {
            return Ok((server, tools));
        }
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_TOOL_PAGES {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let page = server
                .request("tools/list", params, &wait)
                .map_err(|err| failed(&server, "list its tools", err))?;
            let Some(listed) = page["tools"].as_array() else {
                return Err(server.refused("its tool list is not an array"));
            };
            for tool in listed {
                let Some(name) = tool["name"].as_str() else {
                    continue;
                };
                let description = tool["description"]
                    .as_str()
                    .or_else(|| tool["title"].as_str())
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| text.chars().take(MAX_DESCRIPTION_CHARS).collect())
                    .unwrap_or_else(|| {
                        format!("The `{name}` tool of the {} MCP server.", server.name)
                    });
                let input_schema = match &tool["inputSchema"] {
                    schema @ Value::Object(_) => schema.clone(),
                    _ => json!({"type": "object"}),
                };
                tools.push(RemoteTool {
                    name: name.to_string(),
                    description,
                    input_schema,
                });
            }
            match page["nextCursor"].as_str() {
                Some(next) if !next.is_empty() && cursor.as_deref() != Some(next) => {
                    cursor = Some(next.to_string());
                }
                _ => return Ok((server, tools)),
            }
        }
        Err(server.refused(&format!(
            "its tool list has more than {MAX_TOOL_PAGES} pages"
        )))
    }

    /// `initialize` and `notifications/initialized`, after which the
    /// server takes other requests. Returns the initialize result.
    fn handshake(&self, wait: &Wait<'_>, timeout: Duration) -> Result<Value, StartError> {
        let initialized = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "socium", "version": env!("CARGO_PKG_VERSION")},
                }),
                wait,
            )
            .map_err(|err| self.not_started("initialize", err, timeout))?;
        let answered = initialized["protocolVersion"].as_str().unwrap_or_default();
        let Some(version) = PROTOCOL_VERSIONS
            .iter()
            .find(|version| **version == answered)
        else {
            return Err(self.refused(&format!(
                "it answered with protocol version `{answered}`, and Socium speaks {}",
                PROTOCOL_VERSIONS.join(", ")
            )));
        };
        if let Transport::Http(http) = &self.transport {
            http.negotiated(version);
        }
        self.notify("notifications/initialized", None, Some(wait.deadline))
            .map_err(|err| self.not_started("initialize", err, timeout))?;
        Ok(initialized)
    }

    /// How a server that started is doing when it no longer serves: it
    /// exited, or it wants a login. `None` while it may still answer.
    fn ended(&self) -> Option<McpServerState> {
        match &self.transport {
            Transport::Stdio(stdio) => {
                let status = stdio.exited()?;
                Some(McpServerState::Failed {
                    reason: self.redact(&format!(
                        "it exited after it started ({status}).{}",
                        stdio.stderr_tail()
                    )),
                })
            }
            Transport::Http(http) => http.needs_login().then_some(McpServerState::NeedsLogin),
        }
    }

    /// Why the server did not start after it failed to `what`.
    fn not_started(&self, what: &str, err: RequestError, timeout: Duration) -> StartError {
        if let RequestError::NeedsLogin = err {
            return StartError::NeedsLogin;
        }
        let reason = match err {
            RequestError::TimedOut => {
                format!("did not {what} within {} seconds", timeout.as_secs())
            }
            RequestError::Refused(message) => format!("refused to {what}: {message}"),
            RequestError::Malformed => format!("sent a malformed answer when asked to {what}"),
            RequestError::Transport(message) => message,
            RequestError::SessionEnded => "ended the session it had just started".into(),
            RequestError::Gone | RequestError::Cancelled | RequestError::NeedsLogin => {
                format!(
                    "stopped before it could {what}{}",
                    self.transport.exit_status()
                )
            }
        };
        StartError::NotStarted {
            origin: self.transport.origin().map(str::to_string),
            reason: self.redact(&format!("it {reason}.{}", self.transport.stderr_tail())),
        }
    }

    /// Why a server whose answers Socium cannot use did not start.
    fn refused(&self, problem: &str) -> StartError {
        StartError::NotStarted {
            origin: None,
            reason: self.redact(&format!("{problem}.")),
        }
    }

    /// `text` without the values of the server's table and, for a remote
    /// server, the OAuth tokens it was sent. Everything Socium builds from
    /// what a server answered goes through this before it is shown or cut.
    fn redact(&self, text: &str) -> String {
        self.transport.redact(&self.secrets.redact(text))
    }

    fn request(&self, method: &str, params: Value, wait: &Wait<'_>) -> Result<Value, RequestError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let responses = self.transport.send(id, &message)?;
        let response = loop {
            match responses.recv_timeout(POLL) {
                Ok(reply) => break reply,
                Err(mpsc::RecvTimeoutError::Disconnected) => break Err(RequestError::Gone),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Some(stop) = wait.stopped() {
                self.transport.forget(id);
                // A client must not cancel its initialize request; the
                // server is stopped instead.
                if method != "initialize" {
                    let reason = match stop {
                        RequestError::Cancelled => "The user cancelled the request.",
                        _ => "The request took too long.",
                    };
                    let _ = self.notify(
                        "notifications/cancelled",
                        Some(json!({"requestId": id, "reason": reason})),
                        None,
                    );
                }
                break Err(stop);
            }
        }?;
        match (response.get("result"), response.get("error")) {
            (Some(result), None) => Ok(result.clone()),
            (None, Some(error)) => Err(RequestError::Refused(
                error["message"]
                    .as_str()
                    .unwrap_or("no message given")
                    .to_string(),
            )),
            _ => Err(RequestError::Malformed),
        }
    }

    fn notify(
        &self,
        method: &str,
        params: Option<Value>,
        deadline: Option<Instant>,
    ) -> Result<(), RequestError> {
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.transport.notify(&message, deadline)
    }

    /// Starts a new session after the server ended one (HTTP 404), unless
    /// another call already did.
    fn restart_session(&self, control: &TaskControl) -> Result<(), StartError> {
        let Transport::Http(http) = &self.transport else {
            return Ok(());
        };
        let _restarting = lock(&self.restarting);
        if !http.session_ended() {
            return Ok(());
        }
        let wait = Wait {
            deadline: Instant::now() + STARTUP_TIMEOUT,
            control,
        };
        self.handshake(&wait, STARTUP_TIMEOUT).map(drop)
    }

    /// Runs `tool` with `arguments` until it answers, the server's
    /// `tool_timeout_secs` pass, or the task is cancelled, and turns its
    /// content into the text the model reads.
    fn call(
        &self,
        tool: &str,
        arguments: &Value,
        control: &TaskControl,
    ) -> Result<String, ToolError> {
        let arguments = match arguments {
            Value::Null => json!({}),
            object @ Value::Object(_) => object.clone(),
            _ => {
                return Err(ToolError::Message(
                    "The arguments of an MCP tool must be a JSON object".into(),
                ));
            }
        };
        // A session the server ended meanwhile, as when it answered a
        // notification with 404, is started again before the call.
        if self.restart_session(control).is_err() {
            return Err(ToolError::Message(format!(
                "The {} MCP server ended its session (HTTP 404), and a new session could not be started, so the call was not sent; reload the settings or restart Socium",
                self.name
            )));
        }
        let wait = Wait {
            deadline: Instant::now() + self.call_timeout,
            control,
        };
        let unknown = "so whether it ran is unknown";
        let result = self
            .request(
                "tools/call",
                json!({"name": tool, "arguments": arguments}),
                &wait,
            )
            .map_err(|err| {
                ToolError::Message(self.redact(&match err {
                    RequestError::Cancelled => format!(
                        "Cancelled; the {} MCP server was asked to stop the call",
                        self.name
                    ),
                    RequestError::TimedOut => format!(
                        "The {} MCP server did not answer within {} seconds and was asked to stop the call, {unknown}",
                        self.name,
                        self.call_timeout.as_secs()
                    ),
                    RequestError::Refused(message) => {
                        format!("The {} MCP server refused the call: {message}", self.name)
                    }
                    RequestError::Malformed => format!(
                        "The {} MCP server sent a malformed answer, {unknown}",
                        self.name
                    ),
                    RequestError::Transport(message) => {
                        format!("The {} MCP server {message}", self.name)
                    }
                    // The spec has the client start a new session. The call
                    // is not sent again: the model decides.
                    RequestError::SessionEnded => {
                        match self.restart_session(control) {
                            Ok(_) => format!(
                                "The {} MCP server had ended its session (HTTP 404), so the call did not run; Socium started a new session, so call the tool again",
                                self.name
                            ),
                            Err(_) => format!(
                                "The {} MCP server ended its session (HTTP 404), so the call did not run, and a new session could not be started; reload the settings or restart Socium",
                                self.name
                            ),
                        }
                    }
                    RequestError::NeedsLogin => needs_login(&self.name),
                    // A child's standard error stays out: it is for the
                    // user, and may hold what the server was given.
                    RequestError::Gone => format!(
                        "The {} MCP server stopped before answering{}",
                        self.name,
                        self.transport.exit_status()
                    ),
                }))
            })?;
        if !result["content"].is_array() {
            return Err(ToolError::Message(format!(
                "The {} MCP server sent a result without content, {unknown}",
                self.name
            )));
        }
        let text = result_text(&result);
        if result["isError"] == true {
            return Err(ToolError::Message(self.redact(&text)));
        }
        Ok(text)
    }
}

/// What the user sees when a remote server wants a login Socium does not
/// have.
fn needs_login(server: &str) -> String {
    format!("MCP server {server} needs you to log in: run socium mcp login {server}")
}

/// Why a server did not start.
#[derive(Debug)]
enum StartError {
    /// A remote server refused Socium, which has no token or could not
    /// renew it.
    NeedsLogin,
    NotStarted {
        /// Where a remote server is, when it was reached.
        origin: Option<String>,
        /// A sentence that says what went wrong, with the end of a child's
        /// standard error.
        reason: String,
    },
}

impl StartError {
    /// The warning the user sees at the first turn.
    fn warning(&self, server: &str) -> String {
        match self {
            Self::NeedsLogin => needs_login(server),
            Self::NotStarted { origin, reason } => {
                let at = match origin {
                    Some(origin) => format!(" at {origin}"),
                    None => String::new(),
                };
                format!("MCP server {server}{at} was not started: {reason}")
            }
        }
    }

    fn state(self) -> McpServerState {
        match self {
            Self::NeedsLogin => McpServerState::NeedsLogin,
            Self::NotStarted { reason, .. } => McpServerState::Failed { reason },
        }
    }
}

/// One configured MCP server as `/mcp` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerStatus {
    pub name: String,
    pub transport: McpServerTransport,
    pub state: McpServerState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerState {
    /// The startup thread has not finished.
    Starting,
    /// Started; the tools it offers, by their names on the server.
    Connected { tools: Vec<String> },
    /// A remote server wants `socium mcp login <name>`.
    NeedsLogin,
    /// Did not start, as a sentence such as "it did not initialize within
    /// 30 seconds."
    Failed { reason: String },
    /// Not started because of the settings.
    Disabled { reason: McpDisabled },
}

/// A server running as a child process that speaks newline-delimited
/// JSON-RPC on stdin and stdout. It is stopped when this is dropped.
struct StdioTransport {
    child: Mutex<Child>,
    /// The thread that started the child waits on this until the child is
    /// reaped: bubblewrap's `--die-with-parent` ends the sandbox when the
    /// thread that started it ends, not the process.
    parent: Option<(mpsc::Sender<()>, std::thread::JoinHandle<()>)>,
    outbox: Outbox,
    /// Callers waiting for a response, by request ID; `None` once the
    /// server's output closed.
    waiters: Waiters,
    stderr: Arc<StderrTail>,
}

/// The last lines of a server's standard error, and whether the stream
/// has closed, as it does when the server ends.
#[derive(Default)]
struct StderrTail {
    lines: Mutex<VecDeque<String>>,
    closed: Mutex<bool>,
    closing: Condvar,
}

/// How long the lines of a server that ended may take to come in.
const STDERR_SETTLE: Duration = Duration::from_millis(20);

/// The command that starts `argv` on the host. It runs in `/`: a launcher
/// such as `python3 -m` or `npx` would load the workspace's modules and
/// packages first.
fn host_command(
    argv: &[String],
    env: &BTreeMap<String, String>,
    workspace_root: &Path,
    host_env: &dyn Fn(&str) -> Option<String>,
) -> Command {
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]).current_dir("/").env_clear();
    for variable in INHERITED_VARIABLES {
        let value = match *variable {
            "PATH" => host_env(variable).map(|path| path_outside(&path, workspace_root)),
            _ => host_env(variable),
        };
        if let Some(value) = value {
            command.env(variable, value);
        }
    }
    command.envs(env);
    command
}

/// The command that starts `argv` in the read-only sandbox (ADR 33): the
/// workspace read-only and the cwd, its sensitive files as of now hidden, no
/// network, a private HOME and tmp, the runtimes and the server's own
/// `read_only` directories, and none of the `[sandbox]` settings. `env`
/// reaches the server through bubblewrap's descriptor, never bubblewrap's
/// own environment or command line. `Err` says why it cannot start, as a
/// sentence; nothing then falls back to the host or to asking.
fn sandboxed_command(
    argv: &[String],
    env: &BTreeMap<String, String>,
    sandbox: &McpSandboxSettings,
    workspace_root: &Path,
    sensitive: &PathPatterns,
    host_env: &dyn Fn(&str) -> Option<String>,
    wait: &Wait<'_>,
) -> Result<Command, String> {
    // Checked again here: the workspace was not known when the settings
    // were read, and links may have changed since. The sandbox then mounts
    // each from a descriptor opened at the path checked here.
    for mount in &sandbox.read_only {
        let source = mount.source.display();
        let now = mount
            .source
            .canonicalize()
            .map_err(|err| format!("its read_only directory {source} cannot be read: {err}."))?;
        let moved = |now: &Path| {
            format!(
                "its read_only directory {} now leads to {}, not where it did when the settings were read.",
                mount.alias.as_deref().unwrap_or(&mount.source).display(),
                now.display()
            )
        };
        if now != mount.source {
            return Err(moved(&now));
        }
        if let Some(alias) = &mount.alias
            && let Ok(now) = alias.canonicalize()
            && now != mount.source
        {
            return Err(moved(&now));
        }
        if let Some(reason) =
            crate::settings::mcp_mount_refusal(mount, Some(workspace_root), host_env)
        {
            return Err(format!("its read_only directory {reason}."));
        }
    }
    let hidden = crate::access::sensitive_files(workspace_root, sensitive, wait.deadline, &|| {
        wait.control.is_cancelled()
    })
    .map_err(|reason| {
        format!(
            "the files matching approval.sensitive could not be hidden from it: {reason}. A sandboxed server never runs with them in view."
        )
    })?;
    let unavailable = |err: std::io::Error| format!("the sandbox could not start it: {err}.");
    let mut policy =
        ExecutionPolicy::new(workspace_root, WorkspaceAccess::ReadOnly).map_err(unavailable)?;
    policy.read_only = sandbox.read_only.clone();
    policy.env = env
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    policy.hidden = hidden;
    // Its program comes from a system runtime or its read_only
    // directories, never from the workspace the model can write.
    policy.workspace_programs = false;
    let cwd = policy.workspace.clone();
    SandboxExecutor::command(&policy, argv, &cwd).map_err(unavailable)
}

impl StdioTransport {
    /// Starts `command`, whose program `argv[0]` names. Its standard error
    /// is kept without `secrets`. `Err` says why, as a sentence.
    fn spawn(mut command: Command, program: &str, secrets: Secrets) -> Result<Self, String> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own process group, so a launcher such as npx or uvx goes down
        // with the server it started.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let (started, spawned) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let parent = std::thread::spawn(move || {
            let child = command.spawn();
            // Its descriptors, such as the sandbox's, are not needed now.
            drop(command);
            let running = child.is_ok();
            let _ = started.send(child);
            if running {
                // Until drop has reaped the child.
                let _ = released.recv();
            }
        });
        let mut child = spawned
            .recv()
            .map_err(|_| format!("`{program}` could not run: its start panicked."))?
            .map_err(|err| format!("`{program}` could not run: {err}."))?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            unreachable!("all three streams are piped");
        };
        let (sender, lines) = mpsc::sync_channel(OUTBOX_MESSAGES);
        let transport = Self {
            child: Mutex::new(child),
            parent: Some((release, parent)),
            outbox: Arc::new(Mutex::new(Some(sender))),
            waiters: Arc::new(Mutex::new(Some(HashMap::new()))),
            stderr: Arc::new(StderrTail::default()),
        };
        std::thread::spawn(move || write_messages(stdin, &lines));
        let (waiters, outbox) = (transport.waiters.clone(), transport.outbox.clone());
        std::thread::spawn(move || read_messages(stdout, &waiters, &outbox));
        let kept = transport.stderr.clone();
        std::thread::spawn(move || {
            keep_stderr_tail(stderr, &kept.lines, &secrets);
            *lock(&kept.closed) = true;
            kept.closing.notify_all();
        });
        Ok(transport)
    }

    fn send(&self, id: u64, message: &Value) -> Result<mpsc::Receiver<Reply>, RequestError> {
        let (sender, responses) = mpsc::channel();
        match lock(&self.waiters).as_mut() {
            Some(waiters) => waiters.insert(id, sender),
            None => return Err(RequestError::Gone),
        };
        if let Err(err) = self.queue(message) {
            self.forget(id);
            return Err(err);
        }
        Ok(responses)
    }

    /// Hands `message` to the writer thread without waiting for it.
    fn queue(&self, message: &Value) -> Result<(), RequestError> {
        let mut line = message.to_string().into_bytes();
        line.push(b'\n');
        match lock(&self.outbox).as_ref() {
            Some(outbox) => outbox.try_send(line).map_err(|_| RequestError::Gone),
            None => Err(RequestError::Gone),
        }
    }

    fn forget(&self, id: u64) {
        if let Some(waiters) = lock(&self.waiters).as_mut() {
            waiters.remove(&id);
        }
    }

    fn exit_status(&self) -> String {
        // The reader may see the pipe close a moment before the exit is
        // visible.
        match self.wait_exited(Duration::from_millis(100)) {
            Some(status) => format!(" ({status})"),
            None => String::new(),
        }
    }

    /// How the child ended, waiting up to `timeout` for it to, on a
    /// descriptor that becomes readable when it exits.
    #[cfg(target_os = "linux")]
    fn wait_exited(&self, timeout: Duration) -> Option<ExitStatus> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        if let Some(status) = self.exited() {
            return Some(status);
        }
        let pid = libc::pid_t::try_from(lock(&self.child).id()).ok()?;
        // SAFETY: pidfd_open takes a process ID and flags and returns a new
        // descriptor or -1. The child is not reaped before drop, so its ID
        // still names it.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        let fd = std::os::fd::RawFd::try_from(fd)
            .ok()
            .filter(|fd| *fd >= 0)?;
        // SAFETY: `fd` was just returned by pidfd_open and nothing else owns
        // it.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return self.exited();
            }
            let mut poll = libc::pollfd {
                fd: fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // Rounded up, so the wait lasts at least `timeout`.
            let millis = i32::try_from(left.as_micros().div_ceil(1000)).unwrap_or(i32::MAX);
            // SAFETY: one pollfd that lives through the call.
            let ready = unsafe { libc::poll(&mut poll, 1, millis) };
            if ready > 0 {
                return self.exited();
            }
            if ready < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                return self.exited();
            }
        }
    }

    /// As on Linux, but by asking again: there is no descriptor for a
    /// child's exit to wait on.
    #[cfg(not(target_os = "linux"))]
    #[expect(
        clippy::disallowed_methods,
        reason = "outside Linux no descriptor becomes ready when a child exits"
    )]
    fn wait_exited(&self, timeout: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.exited() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// How the child ended, once it has. It stays unreaped, so its process
    /// ID, and with it the ID of its group, cannot be reused before drop
    /// signals the group.
    #[cfg(unix)]
    fn exited(&self) -> Option<ExitStatus> {
        use std::os::unix::process::ExitStatusExt;
        let pid = libc::id_t::try_from(lock(&self.child).id()).ok()?;
        // SAFETY: an all-zero siginfo_t is valid, and waitid only writes
        // into it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: WNOWAIT leaves the child for Child::wait in drop.
        let found = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        // SAFETY: waitid filled in the fields of a child state change.
        if found != 0 || unsafe { info.si_pid() } == 0 {
            return None;
        }
        // SAFETY: as above.
        let status = unsafe { info.si_status() };
        // Back to the form waitpid reports, which ExitStatus reads.
        Some(ExitStatus::from_raw(if info.si_code == libc::CLD_EXITED {
            (status & 0xff) << 8
        } else {
            status & 0x7f
        }))
    }

    #[cfg(not(unix))]
    fn exited(&self) -> Option<ExitStatus> {
        lock(&self.child).try_wait().ok().flatten()
    }

    /// Closes the child's input, which tells it to exit.
    fn close_input(&self) {
        lock(&self.outbox).take();
    }

    /// Whether the child exits within `grace`.
    fn exits_within(&self, grace: Duration) -> bool {
        self.wait_exited(grace).is_some()
    }

    fn stderr_tail(&self) -> String {
        // Lines written just before an exit may still be on their way: the
        // stream closes once the server has ended and they are in.
        let closed = lock(&self.stderr.closed);
        drop(
            self.stderr
                .closing
                .wait_timeout_while(closed, STDERR_SETTLE, |closed| !*closed),
        );
        let lines = lock(&self.stderr.lines);
        if lines.is_empty() {
            return String::new();
        }
        format!(
            " Its last standard error lines:\n{}",
            lines.iter().cloned().collect::<Vec<_>>().join("\n")
        )
    }
}

impl Drop for StdioTransport {
    /// Closes the input and gives the child a moment to exit, then sends
    /// its group SIGTERM, then SIGKILL, which also ends what a launcher
    /// started.
    fn drop(&mut self) {
        self.close_input();
        #[cfg(unix)]
        {
            let group = i32::try_from(lock(&self.child).id()).ok();
            let signal = |signal| {
                if let Some(pid) = group {
                    // SAFETY: kill only sends a signal. The child is not
                    // reaped yet, so the group ID is still its own.
                    unsafe {
                        libc::kill(-pid, signal);
                    }
                }
            };
            if !self.exits_within(STOP_GRACE) {
                signal(libc::SIGTERM);
                self.exits_within(STOP_GRACE);
            }
            signal(libc::SIGKILL);
        }
        {
            let mut child = lock(&self.child);
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some((release, parent)) = self.parent.take() {
            drop(release);
            let _ = parent.join();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

/// `path` without relative entries and entries in the workspace, so the
/// workspace cannot provide the program a server's command names.
fn path_outside(path: &str, workspace_root: &Path) -> String {
    let workspace = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    std::env::split_paths(path)
        .filter(|entry| {
            let resolved = entry.canonicalize().unwrap_or_else(|_| entry.clone());
            entry.is_absolute()
                && !resolved.starts_with(&workspace)
                && !entry.starts_with(workspace_root)
        })
        .map(|entry| entry.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":")
}

/// Writes queued lines to a child's input until the queue closes or the
/// child stops reading. A blocked write holds no lock, so cancelling and
/// stopping still work.
fn write_messages(mut stdin: ChildStdin, lines: &mpsc::Receiver<Vec<u8>>) {
    for line in lines {
        if stdin.write_all(&line).and_then(|()| stdin.flush()).is_err() {
            break;
        }
    }
}

/// Routes each response to the caller waiting for its ID and answers the
/// server's own requests until the server's output closes; then every
/// waiting caller is told the server is gone.
fn read_messages(stdout: impl Read, waiters: &Waiters, outbox: &Outbox) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader)
            .take(MAX_MESSAGE_BYTES as u64)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => break,
            Ok(_) if line.len() >= MAX_MESSAGE_BYTES && !line.ends_with(b"\n") => break,
            Ok(_) => {}
        }
        // Anything else, such as a stray log line, is skipped.
        let messages = match serde_json::from_slice::<Value>(&line) {
            Ok(Value::Object(message)) => vec![message],
            Ok(Value::Array(batch)) => batch
                .into_iter()
                .filter_map(|message| match message {
                    Value::Object(message) => Some(message),
                    _ => None,
                })
                .collect(),
            _ => continue,
        };
        for message in messages {
            route_message(message, waiters, outbox);
        }
    }
    lock(waiters).take();
}

fn route_message(message: serde_json::Map<String, Value>, waiters: &Waiters, outbox: &Outbox) {
    match (message.get("method"), message.get("id")) {
        (Some(method), Some(id)) => {
            // The client offers no capabilities, so it answers only pings.
            let reply = if method == "ping" {
                json!({"jsonrpc": "2.0", "id": id, "result": {}})
            } else {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}})
            };
            let mut line = reply.to_string().into_bytes();
            line.push(b'\n');
            // Never waits: a server that sends requests without reading
            // the answers loses them instead of stopping the reader.
            if let Some(outbox) = lock(outbox).as_ref() {
                let _ = outbox.try_send(line);
            }
        }
        (Some(_), None) => {}
        (None, Some(id)) => {
            let waiter = id.as_u64().and_then(|id| {
                lock(waiters)
                    .as_mut()
                    .and_then(|waiters| waiters.remove(&id))
            });
            if let Some(waiter) = waiter {
                let _ = waiter.send(Ok(Value::Object(message)));
            }
        }
        (None, None) => {}
    }
}

/// Keeps the last lines of standard error without `secrets`, reading at
/// most a line's worth of each line however long it runs. Each is redacted
/// before it is cut short, so no part of a value survives the cut.
fn keep_stderr_tail(stderr: impl Read, lines: &Mutex<VecDeque<String>>, secrets: &Secrets) {
    // A value that starts within the shown characters is read whole.
    let line_bytes = STDERR_LINE_CHARS * 4 + secrets.longest();
    let keep = |line: &[u8], cut: bool| {
        let line = String::from_utf8_lossy(line);
        let mut line = line.trim_end();
        if cut {
            line = secrets.without_partial_end(line.trim_end_matches('\u{fffd}'));
        }
        let line = secrets.redact(line);
        if line.is_empty() {
            return;
        }
        let mut lines = lock(lines);
        if lines.len() == STDERR_LINES {
            lines.pop_front();
        }
        lines.push_back(line.chars().take(STDERR_LINE_CHARS).collect());
    };
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    let mut cut = false;
    loop {
        let (used, ended) = match reader.fill_buf() {
            Ok([]) | Err(_) => break,
            Ok(chunk) => {
                let end = chunk.iter().position(|byte| *byte == b'\n');
                let part = &chunk[..end.unwrap_or(chunk.len())];
                let room = line_bytes.saturating_sub(line.len());
                cut |= part.len() > room;
                line.extend_from_slice(&part[..part.len().min(room)]);
                (end.map_or(chunk.len(), |end| end + 1), end.is_some())
            }
        };
        reader.consume(used);
        if ended {
            keep(&line, cut);
            line.clear();
            cut = false;
        }
    }
    keep(&line, cut);
}

/// The text items of a `tools/call` result, joined; other kinds of content
/// become a placeholder line, and structured content stands in when there
/// is no text.
fn result_text(result: &Value) -> String {
    let parts = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| match item["type"].as_str() {
            Some("text") => item["text"].as_str().unwrap_or_default().to_string(),
            Some("resource_link") => format!(
                "[resource link omitted: {}]",
                item["uri"].as_str().unwrap_or("no URI")
            ),
            Some(kind) => format!("[{kind} content omitted]"),
            None => "[unknown content omitted]".to_string(),
        })
        .collect::<Vec<_>>();
    let has_text = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|item| item["type"] == "text");
    if !has_text && let Some(structured) = result.get("structuredContent") {
        return structured.to_string();
    }
    if parts.is_empty() {
        return "The tool returned no content.".to_string();
    }
    parts.join("\n")
}

/// Where an exposed tool name leads.
pub(crate) struct McpTarget<'a> {
    pub(crate) server: &'a str,
    /// The tool's name on its server.
    pub(crate) remote: &'a str,
    /// Whether the server's `allow` list names the tool.
    pub(crate) allowed: bool,
    /// Whether the server runs in the read-only sandbox (ADR 33).
    pub(crate) sandboxed: bool,
    /// Where a remote server is; `None` for one on the host.
    pub(crate) origin: Option<&'a str>,
}

struct Route {
    server: usize,
    remote: String,
    allowed: bool,
}

/// The servers that started and the tools they offer, and how every
/// server it was given ended up.
#[derive(Default)]
pub(crate) struct McpRegistry {
    servers: Vec<McpServer>,
    routes: HashMap<String, Route>,
    definitions: Vec<ToolDefinition>,
    notices: Notices,
    /// With the index in `servers` of each server that started.
    statuses: Vec<(McpServerStatus, Option<usize>)>,
}

impl McpRegistry {
    /// Starts every server at once and waits for each up to `timeout` or
    /// until `stop` is cancelled. A server that fails is left out with a
    /// warning; so is a tool whose exposed name providers would refuse.
    pub(crate) fn connect(
        servers: &[McpServerSettings],
        workspace_root: &Path,
        sensitive: &PathPatterns,
        host_env: &(dyn Fn(&str) -> Option<String> + Sync),
        tokens: Option<&TokenStore>,
        timeout: Duration,
        stop: &TaskControl,
    ) -> (Self, Vec<String>) {
        let notices = Notices::default();
        let startup = Startup {
            workspace_root,
            sensitive,
            host_env,
            tokens,
            timeout,
            stop,
            notices: &notices,
        };
        let connected = std::thread::scope(|scope| {
            let handles = servers
                .iter()
                .map(|settings| scope.spawn(move || McpServer::connect(settings, startup)))
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| {
                        Err(StartError::NotStarted {
                            origin: None,
                            reason: "its startup panicked.".into(),
                        })
                    })
                })
                .collect::<Vec<_>>()
        });
        let mut registry = Self::default();
        registry.notices = notices;
        let mut warnings = Vec::new();
        let mut all_bytes = 0;
        for (settings, result) in servers.iter().zip(connected) {
            let transport = McpServerTransport::of(&settings.transport);
            let (server, tools) = match result {
                Ok(connected) => connected,
                Err(err) => {
                    warnings.push(err.warning(&settings.name));
                    registry.statuses.push((
                        McpServerStatus {
                            name: settings.name.clone(),
                            transport,
                            state: err.state(),
                        },
                        None,
                    ));
                    continue;
                }
            };
            let mut offered = Vec::new();
            let index = registry.servers.len();
            let (mut bytes, mut left_out) = (0, 0);
            for tool in tools {
                let exposed = format!("{PREFIX}{}__{}", settings.name, tool.name);
                let valid = exposed.len() <= MAX_TOOL_NAME
                    && exposed
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
                // A tool name is the server's, so it may repeat what the
                // server was given.
                if !valid {
                    warnings.push(server.redact(&format!(
                        "MCP server {}: tool `{}` is not offered, since `{exposed}` is not a tool name model providers accept (at most {MAX_TOOL_NAME} letters, digits, '_' or '-')",
                        settings.name, tool.name
                    )));
                    continue;
                }
                if registry.routes.contains_key(&exposed) {
                    warnings.push(server.redact(&format!(
                        "MCP server {}: tool `{}` is not offered, since another tool is already named `{exposed}`",
                        settings.name, tool.name
                    )));
                    continue;
                }
                if tool.input_schema.to_string().len() > MAX_SCHEMA_BYTES {
                    warnings.push(server.redact(&format!(
                        "MCP server {}: tool `{}` is not offered, since its input schema is larger than {} KiB",
                        settings.name,
                        tool.name,
                        MAX_SCHEMA_BYTES / 1024
                    )));
                    continue;
                }
                let size = tool.description.len() + tool.input_schema.to_string().len();
                if offered.len() == MAX_TOOLS_PER_SERVER
                    || registry.definitions.len() == MAX_MCP_TOOLS
                    || bytes + size > MAX_SERVER_DEFINITION_BYTES
                    || all_bytes + size > MAX_MCP_DEFINITION_BYTES
                {
                    left_out += 1;
                    continue;
                }
                (bytes, all_bytes) = (bytes + size, all_bytes + size);
                registry.definitions.push(ToolDefinition {
                    name: exposed.clone(),
                    description: tool.description,
                    input_schema: tool.input_schema,
                });
                offered.push(tool.name.clone());
                registry.routes.insert(
                    exposed,
                    Route {
                        server: index,
                        allowed: settings.allow.contains(&tool.name),
                        remote: tool.name,
                    },
                );
            }
            if left_out > 0 {
                warnings.push(format!(
                    "MCP server {}: {left_out} tools are not offered, since a server may offer at most {MAX_TOOLS_PER_SERVER} tools and {} KiB of descriptions and schemas, and all servers together {MAX_MCP_TOOLS} tools and {} KiB",
                    settings.name,
                    MAX_SERVER_DEFINITION_BYTES / 1024,
                    MAX_MCP_DEFINITION_BYTES / 1024
                ));
            }
            registry.statuses.push((
                McpServerStatus {
                    name: settings.name.clone(),
                    transport,
                    state: McpServerState::Connected { tools: offered },
                },
                Some(index),
            ));
            registry.servers.push(server);
        }
        (registry, warnings)
    }

    /// Warnings that arose since the last call.
    pub(crate) fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut lock(&self.notices))
    }

    /// A registry where every one of `servers` failed for `reason`.
    pub(crate) fn failed(servers: &[McpServerSettings], reason: &str) -> Self {
        let mut registry = Self::default();
        registry.statuses = servers
            .iter()
            .map(|server| {
                let state = McpServerState::Failed {
                    reason: reason.to_string(),
                };
                let status = McpServerStatus {
                    name: server.name.clone(),
                    transport: McpServerTransport::of(&server.transport),
                    state,
                };
                (status, None)
            })
            .collect();
        registry
    }

    /// How each server it was given ended up, in the order given.
    /// A server that started shows how it is now: one that exited or that
    /// wants a login since is no longer connected.
    pub(crate) fn statuses(&self) -> Vec<McpServerStatus> {
        self.statuses
            .iter()
            .map(
                |(status, started)| match started.and_then(|index| self.servers[index].ended()) {
                    Some(state) => McpServerStatus {
                        state,
                        ..status.clone()
                    },
                    None => status.clone(),
                },
            )
            .collect()
    }

    /// The tools to offer the model.
    pub(crate) fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    pub(crate) fn target(&self, name: &str) -> Option<McpTarget<'_>> {
        let route = self.routes.get(name)?;
        Some(McpTarget {
            server: &self.servers[route.server].name,
            remote: &route.remote,
            allowed: route.allowed,
            sandboxed: self.servers[route.server].sandboxed,
            origin: self.servers[route.server].transport.origin(),
        })
    }

    /// Runs the tool exposed as `name`; `None` when no server offers it.
    pub(crate) fn call(
        &self,
        name: &str,
        input: &Value,
        control: &TaskControl,
    ) -> Option<Result<String, ToolError>> {
        let route = self.routes.get(name)?;
        Some(self.servers[route.server].call(&route.remote, input, control))
    }
}

impl Drop for McpRegistry {
    /// Closes every server's input first, so they exit together rather
    /// than one grace period after another.
    fn drop(&mut self) {
        for server in &self.servers {
            server.transport.close();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const FAKE_SERVER: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/fake_mcp_server.py"
    );

    pub(crate) fn fake_settings(name: &str, mode: &str, allow: &[&str]) -> McpServerSettings {
        let mut command = vec!["python3".to_string(), FAKE_SERVER.to_string()];
        if !mode.is_empty() {
            command.push(mode.to_string());
        }
        McpServerSettings {
            name: name.into(),
            transport: McpTransportSettings::Stdio {
                command,
                env: BTreeMap::from([("FAKE_CONFIGURED".to_string(), "yes".to_string())]),
                sandbox: None,
            },
            allow: allow.iter().map(|tool| tool.to_string()).collect(),
            tool_timeout: Duration::from_secs(600),
        }
    }

    /// The parent's environment plus a provider key, as Socium would see
    /// them.
    pub(crate) fn host_env(name: &str) -> Option<String> {
        match name {
            "ANTHROPIC_API_KEY" => Some("sk-ant-must-not-leak".into()),
            other => crate::host::process_env(other),
        }
    }

    pub(crate) fn connect_fake(servers: &[McpServerSettings]) -> (McpRegistry, Vec<String>) {
        connect_with_tokens(servers, None)
    }

    pub(crate) fn connect_with_tokens(
        servers: &[McpServerSettings],
        tokens: Option<&TokenStore>,
    ) -> (McpRegistry, Vec<String>) {
        let root = tempfile::tempdir().unwrap();
        connect_in(root.path(), servers, &host_env, tokens, STARTUP_TIMEOUT)
    }

    fn call(registry: &McpRegistry, name: &str, input: Value) -> Result<String, String> {
        registry
            .call(name, &input, &TaskControl::default())
            .expect("offered")
            .map_err(|err| err.to_string())
    }

    #[test]
    fn a_server_offers_its_tools_under_prefixed_names_and_answers_calls() {
        let (registry, warnings) = connect_fake(&[fake_settings("fake", "", &["echo"])]);

        let names = registry
            .definitions()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "mcp__fake__echo",
                "mcp__fake__fail",
                "mcp__fake__image",
                "mcp__fake__structured",
                "mcp__fake__slow",
                "mcp__fake__cancelled",
                "mcp__fake__die",
                "mcp__fake__env",
                "mcp__fake__pong",
                "mcp__fake__where",
                "mcp__fake__deaf",
                "mcp__fake__malformed",
                "mcp__fake__empty",
                "mcp__fake__batch",
                "mcp__fake__secret",
                "mcp__fake__expire",
                "mcp__fake__reflect",
            ]
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("`bad.name`"), "{warnings:?}");
        let echo = &registry.definitions()[0];
        assert_eq!(echo.description, "Echoes its text.");
        assert_eq!(echo.input_schema["required"], json!(["text"]));
        assert_eq!(
            registry.definitions()[3].input_schema,
            json!({"type": "object"})
        );
        let target = registry.target("mcp__fake__echo").unwrap();
        assert_eq!(
            (target.server, target.remote, target.allowed),
            ("fake", "echo", true)
        );
        assert!(!registry.target("mcp__fake__fail").unwrap().allowed);
        assert!(
            registry
                .call("mcp__fake__missing", &json!({}), &TaskControl::default())
                .is_none()
        );

        assert_eq!(
            call(&registry, "mcp__fake__echo", json!({"text": "hi"})),
            Ok("echo: hi".into())
        );
        assert_eq!(
            call(&registry, "mcp__fake__fail", json!({})),
            Err("it broke".into())
        );
        assert_eq!(
            call(&registry, "mcp__fake__image", Value::Null),
            Ok("a chart\n[image content omitted]".into())
        );
        assert_eq!(
            call(&registry, "mcp__fake__structured", json!({})),
            Ok(r#"{"temperature":21}"#.into())
        );
        // The client answered the server's ping.
        assert_eq!(
            call(&registry, "mcp__fake__pong", json!({})),
            Ok("[{}]".into())
        );
        assert_eq!(
            call(&registry, "mcp__fake__batch", json!({})),
            Ok("batched".into())
        );
    }

    #[test]
    fn a_server_sees_only_the_fixed_environment() {
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &[])]);

        let names = call(&registry, "mcp__fake__env", json!({})).unwrap();

        let names = names.split(',').collect::<Vec<_>>();
        assert!(names.contains(&"FAKE_CONFIGURED"), "{names:?}");
        assert!(names.contains(&"PATH"), "{names:?}");
        for name in &names {
            assert!(
                ["PATH", "HOME", "USER", "LANG", "FAKE_CONFIGURED"].contains(name)
                    // Python sets this one itself on some platforms.
                    || *name == "LC_CTYPE",
                "the server saw {name}: {names:?}"
            );
        }
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "cancels on purpose while the call is in flight"
    )]
    fn a_cancelled_call_returns_promptly_and_the_server_is_told() {
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &[])]);
        let control = TaskControl::default();
        let canceller = control.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            canceller.cancel();
        });

        let started = Instant::now();
        let result = registry
            .call("mcp__fake__slow", &json!({}), &control)
            .unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        let err = result.unwrap_err().to_string();
        assert!(err.starts_with("Cancelled"), "{err}");
        // The slow call was request 4: initialize, two pages, then the call.
        assert_eq!(
            call(&registry, "mcp__fake__cancelled", json!({})),
            Ok("4".into())
        );
    }

    #[test]
    fn a_server_that_exits_during_a_call_is_reported_without_its_stderr() {
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &[])]);

        let err = call(&registry, "mcp__fake__die", json!({})).unwrap_err();

        assert!(err.contains("The fake MCP server stopped"), "{err}");
        assert!(err.contains("exit status: 3"), "{err}");
        assert!(!err.contains("the database is gone"), "{err}");
        let again = call(&registry, "mcp__fake__echo", json!({"text": "x"})).unwrap_err();
        assert!(again.contains("stopped"), "{again}");
    }

    #[test]
    fn servers_that_fail_to_start_are_skipped_with_a_warning() {
        let root = tempfile::tempdir().unwrap();
        let missing = McpServerSettings {
            transport: McpTransportSettings::Stdio {
                command: vec!["/nonexistent/mcp-server".into()],
                env: BTreeMap::new(),
                sandbox: None,
            },
            ..fake_settings("missing", "", &[])
        };
        let (registry, warnings) = connect_in(
            root.path(),
            &[
                fake_settings("crash", "--crash", &[]),
                fake_settings("hang", "--hang", &[]),
                missing,
                fake_settings("good", "", &[]),
            ],
            &host_env,
            None,
            Duration::from_secs(2),
        );

        assert!(registry.target("mcp__good__echo").is_some());
        assert!(registry.target("mcp__crash__echo").is_none());
        let warnings = warnings
            .into_iter()
            .filter(|warning| !warning.contains("bad.name"))
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(
            warnings[0].starts_with("MCP server crash was not started")
                && warnings[0].contains("error: missing credentials"),
            "{}",
            warnings[0]
        );
        assert!(
            warnings[1].contains("did not initialize within 2 seconds"),
            "{}",
            warnings[1]
        );
        assert!(
            warnings[2].contains("/nonexistent/mcp-server"),
            "{}",
            warnings[2]
        );
    }

    #[test]
    fn the_registry_remembers_how_every_server_ended_up() {
        let root = tempfile::tempdir().unwrap();
        let login = FakeHttpServer::start(&["--oauth"]);
        let missing = McpServerSettings {
            transport: McpTransportSettings::Stdio {
                command: vec!["/nonexistent/mcp-server".into(), "--flag".into()],
                env: BTreeMap::new(),
                sandbox: None,
            },
            ..fake_settings("missing", "", &[])
        };
        let (registry, _) = McpRegistry::connect(
            &[
                fake_settings("good", "", &[]),
                missing,
                fake_settings("hang", "--hang", &[]),
                login.settings("remote", &[], &[]),
            ],
            root.path(),
            &PathPatterns::default(),
            &host_env,
            None,
            Duration::from_secs(2),
            &TaskControl::default(),
        );

        let statuses = registry.statuses();
        let names = statuses.iter().map(|s| s.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, ["good", "missing", "hang", "remote"]);
        assert_eq!(
            statuses[0].transport,
            McpServerTransport::Local {
                program: "python3".into(),
                sandboxed: false,
            }
        );
        // The tool whose exposed name providers refuse is not counted.
        assert_eq!(
            statuses[0].state,
            McpServerState::Connected {
                tools: [
                    "echo",
                    "fail",
                    "image",
                    "structured",
                    "slow",
                    "cancelled",
                    "die",
                    "env",
                    "pong",
                    "where",
                    "deaf",
                    "malformed",
                    "empty",
                    "batch",
                    "secret",
                    "expire",
                    "reflect"
                ]
                .map(String::from)
                .to_vec()
            }
        );
        let McpServerState::Failed { reason } = &statuses[1].state else {
            panic!("{:?}", statuses[1]);
        };
        assert!(
            reason.starts_with("`/nonexistent/mcp-server` could not run: "),
            "{reason}"
        );
        assert_eq!(
            statuses[2].state,
            McpServerState::Failed {
                reason: "it did not initialize within 2 seconds. Its last standard error lines:\nfake server starting".into()
            }
        );
        assert_eq!(
            statuses[3].transport,
            McpServerTransport::Remote {
                url: login.url.clone()
            }
        );
        assert_eq!(statuses[3].state, McpServerState::NeedsLogin);
    }

    #[test]
    fn a_server_that_exits_after_starting_is_no_longer_connected() {
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &[])]);
        assert!(matches!(
            registry.statuses()[0].state,
            McpServerState::Connected { .. }
        ));

        call(&registry, "mcp__fake__die", json!({})).unwrap_err();

        let McpServerState::Failed { reason } = &registry.statuses()[0].state else {
            panic!("{:?}", registry.statuses());
        };
        assert!(
            reason.starts_with("it exited after it started (exit status: 3)."),
            "{reason}"
        );
        assert!(reason.contains("fatal: the database is gone"), "{reason}");
    }

    #[test]
    fn a_failure_the_server_wrote_a_secret_into_is_blanked_in_the_status() {
        let mut settings = fake_settings("crash", "--crash", &[]);
        stdio(&mut settings)
            .1
            .insert("FAKE_SECRET".into(), "s3cret-value-123".into());

        let (registry, _) = connect_fake(&[settings]);

        let McpServerState::Failed { reason } = &registry.statuses()[0].state else {
            panic!("{:?}", registry.statuses());
        };
        assert!(reason.contains("token=[redacted]"), "{reason}");
        assert!(!reason.contains("s3cret"), "{reason}");
    }

    /// ADR 29: a value is redacted before a line is cut short, so one that
    /// the cut would split leaves no start of it in the warning or `/mcp`.
    #[test]
    fn a_secret_across_the_cut_of_a_standard_error_line_leaves_nothing_behind() {
        let mut settings = fake_settings("crash", "--crash", &[]);
        let env = stdio(&mut settings).1;
        env.insert("FAKE_SECRET".into(), "s3cret-value-123".into());
        // "token=" and the x's leave six characters of the value shown.
        let pad = STDERR_LINE_CHARS - "token=".len() - 6;
        env.insert("FAKE_PAD".into(), pad.to_string());

        let (registry, warnings) = connect_fake(&[settings]);

        let McpServerState::Failed { reason } = &registry.statuses()[0].state else {
            panic!("{:?}", registry.statuses());
        };
        for text in [reason, &warnings[0]] {
            assert!(text.contains("token=xxx"), "{text}");
            assert!(!text.contains("s3cret"), "{text}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn dropping_the_registry_ends_the_server() {
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &[])]);
        let Transport::Stdio(stdio) = &registry.servers[0].transport else {
            unreachable!("a child process");
        };
        let pid = lock(&stdio.child).id();
        assert!(Path::new(&format!("/proc/{pid}")).exists());

        drop(registry);

        // Reaped, not left a zombie.
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }

    /// A host server that ignores both its closed input and SIGTERM is
    /// ended by SIGKILL once the second grace period is over.
    #[cfg(target_os = "linux")]
    #[test]
    fn dropping_the_registry_kills_a_server_that_ignores_sigterm() {
        let mut settings = fake_settings("stubborn", "--probe", &[]);
        stdio(&mut settings).0.push("--linger=term".into());
        let (registry, warnings) = connect_fake(&[settings]);
        let Transport::Stdio(stdio) = &registry.servers[0].transport else {
            unreachable!("a child process: {warnings:?}");
        };
        let pid = lock(&stdio.child).id();

        let started = Instant::now();
        drop(registry);
        let took = started.elapsed();

        assert!(took >= STOP_GRACE * 2, "{took:?}");
        assert!(took < STOP_GRACE * 2 + Duration::from_secs(5), "{took:?}");
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }

    /// A start cancelled before the server runs does not run it.
    #[test]
    fn a_cancelled_start_never_runs_the_server() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("ran");
        let settings = fake_settings("fake", &format!("--on-eof={}", marker.display()), &[]);
        let stop = TaskControl::default();
        stop.cancel();

        let (registry, warnings) = McpRegistry::connect(
            &[settings],
            root.path(),
            &PathPatterns::default(),
            &host_env,
            None,
            STARTUP_TIMEOUT,
            &stop,
        );
        drop(registry);

        assert!(
            warnings[0].contains("its start was cancelled before it ran"),
            "{warnings:?}"
        );
        assert!(!marker.exists(), "the server ran");
    }

    fn connect_in(
        root: &Path,
        servers: &[McpServerSettings],
        env: &(dyn Fn(&str) -> Option<String> + Sync),
        tokens: Option<&TokenStore>,
        timeout: Duration,
    ) -> (McpRegistry, Vec<String>) {
        McpRegistry::connect(
            servers,
            root,
            &PathPatterns::new(crate::settings::DEFAULT_SENSITIVE),
            env,
            tokens,
            timeout,
            &TaskControl::default(),
        )
    }

    fn stdio(
        settings: &mut McpServerSettings,
    ) -> (&mut Vec<String>, &mut BTreeMap<String, String>) {
        match &mut settings.transport {
            McpTransportSettings::Stdio { command, env, .. } => (command, env),
            McpTransportSettings::Http { .. } => panic!("not a stdio server"),
        }
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "cancels on purpose while the call is in flight"
    )]
    fn cancel_after(delay: Duration) -> TaskControl {
        let control = TaskControl::default();
        let canceller = control.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            canceller.cancel();
        });
        control
    }

    #[test]
    fn a_server_runs_in_the_root_with_no_workspace_entries_on_its_path() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let path = format!(".:{}:/usr/bin:/bin", bin.display());
        let env = move |name: &str| match name {
            "PATH" => Some(path.clone()),
            other => host_env(other),
        };
        let python = std::env::split_paths(&crate::host::search_path().unwrap())
            .map(|dir| dir.join("python3"))
            .find(|path| path.is_file())
            .unwrap();
        let mut settings = fake_settings("fake", "", &[]);
        stdio(&mut settings).0[0] = python.display().to_string();
        let (registry, warnings) =
            connect_in(root.path(), &[settings], &env, None, STARTUP_TIMEOUT);
        assert!(
            registry.target("mcp__fake__where").is_some(),
            "{warnings:?}"
        );

        let answer = call(&registry, "mcp__fake__where", json!({})).unwrap();

        assert_eq!(answer, "/\n/usr/bin:/bin");
    }

    #[test]
    fn a_module_in_the_workspace_does_not_stand_in_for_the_server() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("ran");
        std::fs::write(
            root.path().join("socium_planted.py"),
            format!(
                "open({:?}, 'w').write('ran')\n",
                marker.display().to_string()
            ),
        )
        .unwrap();
        let mut settings = fake_settings("planted", "", &[]);
        *stdio(&mut settings).0 = vec!["python3".into(), "-m".into(), "socium_planted".into()];

        let (registry, warnings) = connect_in(
            root.path(),
            &[settings],
            &host_env,
            None,
            Duration::from_secs(5),
        );

        assert!(registry.definitions().is_empty());
        assert!(warnings[0].contains("was not started"), "{warnings:?}");
        assert!(!marker.exists());
    }

    #[test]
    fn a_server_that_stops_reading_does_not_hold_up_cancelling_or_stopping() {
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &[])]);
        let Transport::Stdio(stdio) = &registry.servers[0].transport else {
            unreachable!("a child process");
        };
        let pid = lock(&stdio.child).id();
        let deaf = registry
            .call(
                "mcp__fake__deaf",
                &json!({}),
                &cancel_after(Duration::from_millis(200)),
            )
            .unwrap();
        assert!(deaf.unwrap_err().to_string().starts_with("Cancelled"));

        // Far more than a pipe holds, so the write cannot finish.
        let started = Instant::now();
        let big = registry
            .call(
                "mcp__fake__echo",
                &json!({"text": "x".repeat(4 * 1024 * 1024)}),
                &cancel_after(Duration::from_millis(300)),
            )
            .unwrap();
        assert!(big.unwrap_err().to_string().starts_with("Cancelled"));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );

        let started = Instant::now();
        drop(registry);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        #[cfg(target_os = "linux")]
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        let _ = pid;
    }

    #[test]
    fn a_call_that_outlasts_its_timeout_ends_and_is_cancelled() {
        let settings = McpServerSettings {
            tool_timeout: Duration::from_secs(1),
            ..fake_settings("fake", "", &[])
        };
        let (registry, _) = connect_fake(&[settings]);

        let err = call(&registry, "mcp__fake__slow", json!({})).unwrap_err();

        assert!(err.contains("did not answer within 1 seconds"), "{err}");
        assert!(err.contains("whether it ran is unknown"), "{err}");
        assert_eq!(
            call(&registry, "mcp__fake__cancelled", json!({})),
            Ok("4".into())
        );
    }

    #[test]
    fn answers_that_are_not_a_result_are_not_taken_for_success() {
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &[])]);

        let malformed = call(&registry, "mcp__fake__malformed", json!({})).unwrap_err();
        let empty = call(&registry, "mcp__fake__empty", json!({})).unwrap_err();
        let listed = call(&registry, "mcp__fake__echo", json!(["hi"])).unwrap_err();

        assert!(malformed.contains("malformed answer"), "{malformed}");
        assert!(empty.contains("without content"), "{empty}");
        assert!(listed.contains("must be a JSON object"), "{listed}");
    }

    #[test]
    fn a_server_must_speak_a_known_revision_and_lists_tools_only_when_it_offers_them() {
        let (registry, warnings) = connect_fake(&[
            fake_settings("old", "--version=2024-11-05", &[]),
            fake_settings("future", "--version=2099-01-01", &[]),
            fake_settings("toolless", "--no-tools", &[]),
        ]);

        assert!(registry.target("mcp__old__echo").is_some());
        assert!(registry.target("mcp__future__echo").is_none());
        assert!(registry.target("mcp__toolless__echo").is_none());
        let warnings = warnings
            .into_iter()
            .filter(|warning| !warning.contains("bad.name"))
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("protocol version `2099-01-01`"),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_long_description_is_cut_and_a_huge_schema_leaves_its_tool_out() {
        let (registry, warnings) = connect_fake(&[fake_settings("fake", "--big", &[])]);

        let wordy = registry
            .definitions()
            .iter()
            .find(|tool| tool.name == "mcp__fake__wordy")
            .unwrap();
        assert_eq!(wordy.description.chars().count(), MAX_DESCRIPTION_CHARS);
        assert!(registry.target("mcp__fake__huge").is_none());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("`huge`") && warning.contains("64 KiB")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_server_offers_at_most_its_share_of_tools_and_all_servers_together_a_total() {
        let (registry, warnings) = connect_fake(&[
            fake_settings("a", "--many", &[]),
            fake_settings("b", "--many", &[]),
        ]);

        let offered = |server: &str| {
            registry
                .definitions()
                .iter()
                .filter(|tool| tool.name.starts_with(&format!("mcp__{server}__")))
                .count()
        };
        assert_eq!(offered("a"), MAX_TOOLS_PER_SERVER);
        assert_eq!(offered("b"), MAX_MCP_TOOLS - MAX_TOOLS_PER_SERVER);
        for server in ["a", "b"] {
            assert!(
                warnings.iter().any(|warning| warning
                    .starts_with(&format!("MCP server {server}: "))
                    && warning.contains("tools are not offered")),
                "{warnings:?}"
            );
        }
    }

    #[test]
    fn a_server_whose_schemas_are_too_large_together_offers_only_what_fits() {
        let (registry, warnings) = connect_fake(&[fake_settings("heavy", "--heavy", &[])]);

        let size = registry
            .definitions()
            .iter()
            .map(|tool| tool.description.len() + tool.input_schema.to_string().len())
            .sum::<usize>();
        assert!(size <= MAX_SERVER_DEFINITION_BYTES, "{size}");
        assert!(registry.target("mcp__heavy__echo").is_some());
        assert!(registry.target("mcp__heavy__h0").is_some());
        assert!(registry.target("mcp__heavy__h3").is_none());
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("tools are not offered")),
            "{warnings:?}"
        );
    }

    #[test]
    fn values_a_server_was_given_are_blanked_out_of_warnings_and_errors() {
        let secret = |name: &str, mode: &str| {
            let mut settings = fake_settings(name, mode, &[]);
            stdio(&mut settings)
                .1
                .insert("FAKE_SECRET".into(), "s3cret-value-123".into());
            settings
        };
        let (registry, warnings) = connect_fake(&[secret("crash", "--crash"), secret("fake", "")]);

        let err = call(&registry, "mcp__fake__secret", json!({})).unwrap_err();

        let crash = warnings
            .iter()
            .find(|warning| warning.contains("crash"))
            .unwrap();
        assert!(crash.contains("token=[redacted]"), "{crash}");
        assert_eq!(
            err,
            "The fake MCP server refused the call: bad token [redacted]"
        );
    }

    #[test]
    fn initialize_is_not_cancelled_when_it_times_out() {
        let root = tempfile::tempdir().unwrap();
        let log = root.path().join("methods");
        let mut settings = fake_settings("hang", "--hang", &[]);
        stdio(&mut settings)
            .1
            .insert("FAKE_LOG".into(), log.display().to_string());

        let (_, warnings) = connect_in(
            root.path(),
            &[settings],
            &host_env,
            None,
            Duration::from_secs(1),
        );

        assert!(warnings[0].contains("did not initialize"), "{warnings:?}");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "initialize\n");
    }

    #[test]
    fn a_stopping_server_gets_to_finish_after_its_input_closes() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("closed");
        let mode = format!("--on-eof={}", marker.display());
        let (registry, _) = connect_in(
            root.path(),
            &[fake_settings("fake", &mode, &[])],
            &host_env,
            None,
            STARTUP_TIMEOUT,
        );

        drop(registry);

        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "closed\n");
    }

    #[test]
    fn a_startup_that_is_stopped_ends_promptly() {
        let root = tempfile::tempdir().unwrap();
        let stop = cancel_after(Duration::from_millis(200));
        let started = Instant::now();

        let (registry, _) = McpRegistry::connect(
            &[fake_settings("hang", "--hang", &[])],
            root.path(),
            &PathPatterns::default(),
            &host_env,
            None,
            STARTUP_TIMEOUT,
            &stop,
        );

        assert!(registry.definitions().is_empty());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn standard_error_keeps_a_bounded_tail_of_endless_lines() {
        let mut stream = vec![b'x'; 8 * 1024 * 1024];
        stream.extend_from_slice(b"\nlast words\n");
        let lines = Mutex::new(VecDeque::new());

        keep_stderr_tail(stream.as_slice(), &lines, &Secrets::default());

        let lines = lines.into_inner().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "x".repeat(STDERR_LINE_CHARS));
        assert_eq!(lines[1], "last words");
    }

    /// A value that a line's cut would split is replaced whole first, and
    /// one the reader stopped inside leaves no start of it behind.
    #[test]
    fn standard_error_is_redacted_before_it_is_cut_short() {
        let secret = format!("s3cret-{}", "v".repeat(93));
        let secrets = Secrets::of_values([secret.clone()].iter());
        let across = format!("{}{secret}\n", "x".repeat(STDERR_LINE_CHARS - 6));
        // Enough long values before it that the reader stops inside the
        // last one, and what is left once they are replaced is short enough
        // to show where it stopped.
        let many = format!(
            "{}x{secret}",
            secret.repeat(STDERR_LINE_CHARS * 4 / secret.len())
        );
        for line in [across, many] {
            let lines = Mutex::new(VecDeque::new());
            keep_stderr_tail(line.as_bytes(), &lines, &secrets);
            let lines = lines.into_inner().unwrap();
            assert_eq!(lines.len(), 1);
            for start in (MIN_SECRET_CHARS / 2..=secret.len()).map(|end| &secret[..end]) {
                assert!(!lines[0].contains(start), "{start}: {}", lines[0]);
            }
        }
    }

    /// The fake server in its HTTP mode, stopped when dropped.
    pub(crate) struct FakeHttpServer {
        child: Child,
        pub(crate) url: String,
        log: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    impl FakeHttpServer {
        pub(crate) fn start(options: &[&str]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("requests.jsonl");
            let mut child = Command::new("python3")
                .arg(FAKE_SERVER)
                .arg("--http")
                .arg(format!("--log={}", log.display()))
                .args(options)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut line = String::new();
            BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            let port = line.trim().strip_prefix("PORT ").expect("a port");
            Self {
                child,
                url: format!("http://127.0.0.1:{port}/mcp"),
                log,
                _dir: dir,
            }
        }

        pub(crate) fn settings(
            &self,
            name: &str,
            headers: &[(&str, &str)],
            allow: &[&str],
        ) -> McpServerSettings {
            McpServerSettings {
                name: name.into(),
                transport: McpTransportSettings::Http {
                    url: self.url.clone(),
                    headers: headers
                        .iter()
                        .map(|(name, value)| (name.to_string(), value.to_string()))
                        .collect(),
                    oauth: None,
                },
                allow: allow.iter().map(|tool| tool.to_string()).collect(),
                tool_timeout: Duration::from_secs(600),
            }
        }

        /// One entry per HTTP request the server received.
        pub(crate) fn requests(&self) -> Vec<Value> {
            std::fs::read_to_string(&self.log)
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    impl Drop for FakeHttpServer {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    const STREAMS: [&[&str]; 3] = [&[], &["--sse"], &["--sse", "--cr"]];

    #[test]
    fn a_remote_server_answers_in_json_bodies_and_in_event_streams() {
        for options in STREAMS {
            let fake = FakeHttpServer::start(options);
            let (registry, warnings) = connect_fake(&[fake.settings("fake", &[], &["echo"])]);

            assert_eq!(warnings.len(), 1, "{options:?}: {warnings:?}");
            assert!(warnings[0].contains("`bad.name`"), "{warnings:?}");
            assert_eq!(registry.definitions().len(), 17, "{options:?}");
            assert_eq!(
                registry.definitions()[0].input_schema["required"],
                json!(["text"])
            );
            assert_eq!(
                call(&registry, "mcp__fake__echo", json!({"text": "hi"})),
                Ok("echo: hi".into())
            );
            assert_eq!(
                call(&registry, "mcp__fake__fail", json!({})),
                Err("it broke".into())
            );
            assert_eq!(
                call(&registry, "mcp__fake__image", Value::Null),
                Ok("a chart\n[image content omitted]".into())
            );
            assert_eq!(
                call(&registry, "mcp__fake__structured", json!({})),
                Ok(r#"{"temperature":21}"#.into())
            );
            // A ping inside the call's event stream was answered.
            let pings = if options.is_empty() { "[]" } else { "[{}]" };
            assert_eq!(
                call(&registry, "mcp__fake__pong", json!({})),
                Ok(pings.into())
            );

            let requests = fake.requests();
            assert_eq!(
                (
                    &requests[0]["rpc"],
                    &requests[0]["session"],
                    &requests[0]["version"]
                ),
                (&json!("initialize"), &Value::Null, &Value::Null)
            );
            assert_eq!(requests[1]["rpc"], "notifications/initialized");
            for request in &requests[1..] {
                assert_eq!(request["session"], "session-1", "{request}");
                // The version the server chose, not the one Socium offered.
                assert_eq!(request["version"], "2025-03-26", "{request}");
            }
        }
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "cancels on purpose while the call is in flight, then the fake server, another process, reports the notification only in its log"
    )]
    fn a_cancelled_remote_call_returns_promptly_and_the_server_is_told() {
        for options in STREAMS {
            let fake = FakeHttpServer::start(options);
            let (registry, _) = connect_fake(&[fake.settings("fake", &[], &[])]);
            let control = TaskControl::default();
            let canceller = control.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                canceller.cancel();
            });

            let started = Instant::now();
            let result = registry
                .call("mcp__fake__slow", &json!({}), &control)
                .unwrap();

            assert!(started.elapsed() < Duration::from_secs(5), "{options:?}");
            let err = result.unwrap_err().to_string();
            assert!(err.starts_with("Cancelled"), "{err}");
            // The notification travels on its own; give it a moment.
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let told = call(&registry, "mcp__fake__cancelled", json!({}));
                if told == Ok("4".into()) || Instant::now() > deadline {
                    assert_eq!(told, Ok("4".into()), "{options:?}");
                    break;
                }
                std::thread::sleep(POLL);
            }
        }
    }

    #[test]
    fn a_remote_server_that_needs_a_token_gets_it_only_from_the_headers() {
        let fake = FakeHttpServer::start(&["--token=s3cret-token"]);
        let unreachable = McpServerSettings {
            transport: McpTransportSettings::Http {
                url: "http://127.0.0.1:9/mcp".into(),
                headers: BTreeMap::from([(
                    "Authorization".to_string(),
                    "Bearer unused-s3cret".to_string(),
                )]),
                oauth: None,
            },
            ..fake.settings("gone", &[], &[])
        };

        let (registry, warnings) = connect_fake(&[
            fake.settings("anonymous", &[], &[]),
            fake.settings("wrong", &[("Authorization", "Bearer wrong-s3cret")], &[]),
            unreachable,
            fake.settings("right", &[("Authorization", "Bearer s3cret-token")], &[]),
        ]);

        assert!(registry.target("mcp__right__echo").is_some());
        assert!(registry.target("mcp__wrong__echo").is_none());
        let warnings = warnings
            .into_iter()
            .filter(|warning| !warning.contains("bad.name"))
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        let origin = fake.url.trim_end_matches("/mcp");
        assert_eq!(
            warnings[0],
            "MCP server anonymous needs you to log in: run socium mcp login anonymous"
        );
        assert_eq!(
            warnings[1],
            format!(
                "MCP server wrong at {origin} was not started: it refused the Authorization header of its [mcp.servers] table (HTTP 401)."
            )
        );
        assert!(
            warnings[2].starts_with(
                "MCP server gone at http://127.0.0.1:9 was not started: it could not be reached"
            ),
            "{}",
            warnings[2]
        );
        assert_eq!(
            call(&registry, "mcp__right__echo", json!({"text": "hi"})),
            Ok("echo: hi".into())
        );
        call(&registry, "mcp__right__expire", json!({})).unwrap();
        let expired = call(&registry, "mcp__right__echo", json!({"text": "hi"})).unwrap_err();
        for text in warnings.iter().chain([&expired]) {
            assert!(!text.contains("s3cret"), "{text}");
        }
    }

    #[test]
    fn an_expired_remote_session_is_renewed_without_sending_the_call_again() {
        let fake = FakeHttpServer::start(&["--sse"]);
        let (registry, _) = connect_fake(&[fake.settings("fake", &[], &[])]);

        call(&registry, "mcp__fake__expire", json!({})).unwrap();
        let err = call(&registry, "mcp__fake__echo", json!({"text": "x"})).unwrap_err();

        assert!(err.contains("had ended its session (HTTP 404)"), "{err}");
        assert!(err.contains("call the tool again"), "{err}");
        let requests = fake.requests();
        let echoes = |requests: &[Value]| {
            requests
                .iter()
                .filter(|request| request["rpc"] == "tools/call")
                .count()
        };
        let after = requests
            .iter()
            .rposition(|request| request["rpc"] == "initialize")
            .unwrap();
        assert!(after > 2, "a second initialize: {requests:?}");
        assert_eq!(requests[after]["session"], Value::Null);
        assert_eq!(requests[after]["version"], Value::Null);
        // The expire call and the echo the server refused; none after.
        assert_eq!(echoes(&requests[after..]), 0);
        assert_eq!(
            call(&registry, "mcp__fake__echo", json!({"text": "x"})),
            Ok("echo: x".into())
        );
    }

    #[test]
    fn a_header_token_is_blanked_out_with_or_without_its_scheme() {
        let fake = FakeHttpServer::start(&[]);
        let settings = fake.settings("fake", &[("Authorization", "Bearer s3cret-token")], &[]);

        let secrets = Secrets::of(&settings);

        assert_eq!(
            secrets.redact("got Bearer s3cret-token, then s3cret-token"),
            "got [redacted], then [redacted]"
        );
    }

    #[test]
    fn a_remote_server_that_repeats_its_token_has_it_blanked_out() {
        let fake = FakeHttpServer::start(&["--token=s3cret-token"]);
        let (registry, _) = connect_fake(&[fake.settings(
            "fake",
            &[("Authorization", "Bearer s3cret-token")],
            &[],
        )]);

        let err = call(&registry, "mcp__fake__reflect", json!({})).unwrap_err();

        assert_eq!(
            err,
            "The fake MCP server answered HTTP 400 Bad Request: rejected [redacted]"
        );
    }

    #[test]
    fn a_header_token_cut_by_the_quote_limit_leaves_no_part_of_it() {
        let fake = FakeHttpServer::start(&["--token=s3cret-token"]);
        let (registry, _) = connect_fake(&[fake.settings(
            "fake",
            &[("Authorization", "Bearer s3cret-token")],
            &[],
        )]);
        // The token would begin four characters before the quote ends.
        let pad = 300 - "rejected ".len() - "Bearer ".len() - 4;

        for how in ["http", "rpc", "result"] {
            let err = call(
                &registry,
                "mcp__fake__reflect",
                json!({"how": how, "pad": pad}),
            )
            .unwrap_err();
            assert!(!err.contains("s3cr"), "{how}: {err}");
            assert!(err.contains("[redacted]"), "{how}: {err}");
        }
    }

    #[test]
    fn a_remote_server_must_answer_json_or_an_event_stream_and_accept_notifications_with_202() {
        let fake = FakeHttpServer::start(&["--content-type=Application/JSON; charset=utf-8"]);
        let (registry, _) = connect_fake(&[fake.settings("fake", &[], &[])]);
        assert_eq!(
            call(&registry, "mcp__fake__echo", json!({"text": "hi"})),
            Ok("echo: hi".into())
        );

        for (option, problem) in [
            (
                "--content-type=application/json-patch+json",
                "answered with content type 'application/json-patch+json' instead of JSON or an event stream",
            ),
            (
                "--notify-status=200",
                "answered HTTP 200 OK instead of 202 Accepted to a notification",
            ),
        ] {
            let fake = FakeHttpServer::start(&[option]);
            let (registry, warnings) = connect_fake(&[fake.settings("fake", &[], &[])]);
            assert!(registry.target("mcp__fake__echo").is_none(), "{option}");
            assert_eq!(warnings.len(), 1, "{option}: {warnings:?}");
            assert!(warnings[0].contains(problem), "{option}: {warnings:?}");
        }
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the fake server is another process that reports requests only in its log file"
    )]
    fn a_late_404_from_an_ended_session_leaves_the_new_one_alone() {
        let fake = FakeHttpServer::start(&["--sse"]);
        let (registry, _) = connect_fake(&[fake.settings("fake", &[], &[])]);

        let late = std::thread::scope(|scope| {
            // Sent in session-1, answered 404 once session-2 has started.
            let late = scope.spawn(|| call(&registry, "mcp__fake__slow", json!({"then": "404"})));
            let deadline = Instant::now() + Duration::from_secs(10);
            while !fake.requests().iter().any(|r| r["rpc"] == "tools/call") {
                assert!(Instant::now() < deadline, "the slow call never arrived");
                std::thread::sleep(POLL);
            }
            call(&registry, "mcp__fake__expire", json!({})).unwrap();
            let err = call(&registry, "mcp__fake__echo", json!({"text": "x"})).unwrap_err();
            assert!(err.contains("call the tool again"), "{err}");
            late.join().unwrap()
        });

        assert!(late.unwrap_err().contains("HTTP 404"));
        assert_eq!(
            call(&registry, "mcp__fake__echo", json!({"text": "y"})),
            Ok("echo: y".into())
        );
        let requests = fake.requests();
        let initializes = requests
            .iter()
            .filter(|request| request["rpc"] == "initialize")
            .count();
        assert_eq!(initializes, 2, "{requests:?}");
        assert_eq!(requests.last().unwrap()["session"], "session-2");
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "the fake server is another process that reports requests only in its log file, and the last wait is for nothing more to arrive"
    )]
    fn a_session_a_notification_found_ended_is_started_again_before_the_next_call() {
        let fake = FakeHttpServer::start(&[]);
        let (registry, _) = connect_fake(&[fake.settings("fake", &[], &[])]);
        let control = TaskControl::default();

        std::thread::scope(|scope| {
            let slow = scope.spawn(|| registry.call("mcp__fake__slow", &json!({}), &control));
            let deadline = Instant::now() + Duration::from_secs(10);
            while !fake.requests().iter().any(|r| r["rpc"] == "tools/call") {
                assert!(Instant::now() < deadline, "the slow call never arrived");
                std::thread::sleep(POLL);
            }
            call(&registry, "mcp__fake__expire", json!({})).unwrap();
            // Its notifications/cancelled finds the session gone.
            control.cancel();
            assert!(slow.join().unwrap().unwrap().is_err());
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !fake
            .requests()
            .iter()
            .any(|r| r["rpc"] == "notifications/cancelled")
        {
            assert!(Instant::now() < deadline, "no cancellation");
            std::thread::sleep(POLL);
        }
        std::thread::sleep(Duration::from_millis(300));

        assert_eq!(
            call(&registry, "mcp__fake__echo", json!({"text": "x"})),
            Ok("echo: x".into())
        );
        let requests = fake.requests();
        let cancelled = requests
            .iter()
            .position(|r| r["rpc"] == "notifications/cancelled")
            .unwrap();
        let rest = requests[cancelled + 1..]
            .iter()
            .map(|r| r["rpc"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(
            rest,
            ["initialize", "notifications/initialized", "tools/call"],
            "{requests:?}"
        );
    }

    #[test]
    fn dropping_the_registry_ends_the_remote_session() {
        let fake = FakeHttpServer::start(&[]);
        let (registry, _) = connect_fake(&[fake.settings("fake", &[], &[])]);
        let origin = fake.url.trim_end_matches("/mcp");
        assert_eq!(
            registry.target("mcp__fake__echo").unwrap().origin,
            Some(origin)
        );

        drop(registry);

        let requests = fake.requests();
        let last = requests.last().unwrap();
        assert_eq!(
            (&last["http"], &last["session"]),
            (&json!("DELETE"), &json!("session-1"))
        );
    }

    /// Servers with `sandbox = "read-only"` (ADR 33). These run in the real
    /// sandbox: missing enforcement fails them, as in `execution::tests`.
    #[cfg(target_os = "linux")]
    pub(crate) mod sandboxed {
        use super::*;
        use crate::settings::{McpSandboxSettings, ReadOnlyMount};
        use std::fs;

        /// The fake server as one executable script. Its `#!` line names
        /// python3 by its absolute path, since a build sandbox has no
        /// `/usr/bin/env`.
        fn fake_server_script() -> String {
            let python = std::env::split_paths(&crate::host::search_path().unwrap())
                .map(|dir| dir.join("python3"))
                .find(|path| path.is_file())
                .unwrap();
            format!(
                "#!{}\n{}",
                python.display(),
                fs::read_to_string(FAKE_SERVER).unwrap()
            )
        }

        /// The fixtures directory, which holds the fake server.
        pub(crate) fn fixtures() -> std::path::PathBuf {
            Path::new(FAKE_SERVER)
                .parent()
                .unwrap()
                .canonicalize()
                .unwrap()
        }

        /// The fake server with its probe tools in the sandbox, `options`
        /// after `--probe`, `env` set for it, and the fixtures directory as
        /// its `read_only`.
        pub(crate) fn sandboxed_settings(
            name: &str,
            options: &[&str],
            env: &[(&str, &str)],
        ) -> McpServerSettings {
            let mut command = vec![
                "python3".to_string(),
                fixtures().join("fake_mcp_server.py").display().to_string(),
                "--probe".to_string(),
            ];
            command.extend(options.iter().map(|option| option.to_string()));
            McpServerSettings {
                name: name.into(),
                transport: McpTransportSettings::Stdio {
                    command,
                    env: env
                        .iter()
                        .map(|(name, value)| (name.to_string(), value.to_string()))
                        .collect(),
                    sandbox: Some(McpSandboxSettings {
                        read_only: vec![ReadOnlyMount {
                            source: fixtures(),
                            alias: None,
                        }],
                    }),
                },
                allow: Vec::new(),
                tool_timeout: Duration::from_secs(600),
            }
        }

        /// A host environment with only HOME, set to `home`: a sandboxed
        /// server reads none of it, and Socium's configuration is then
        /// under `home`.
        fn only_home(home: &Path) -> impl Fn(&str) -> Option<String> + Sync + use<> {
            let home = home.display().to_string();
            move |name: &str| (name == "HOME").then(|| home.clone())
        }

        fn connect_sandboxed(
            root: &Path,
            home: &Path,
            servers: &[McpServerSettings],
        ) -> (McpRegistry, Vec<String>) {
            connect_in(root, servers, &only_home(home), None, STARTUP_TIMEOUT)
        }

        /// A mark for a command line that no other process has.
        fn unique_tag(what: &str) -> String {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            format!(
                "socium-mcp-{what}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )
        }

        /// Processes whose command line holds `tag`, by host process ID.
        fn processes_with(tag: &str) -> Vec<u32> {
            fs::read_dir("/proc")
                .unwrap()
                .flatten()
                .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
                .filter(|pid| {
                    fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|line| {
                        line.windows(tag.len())
                            .any(|window| window == tag.as_bytes())
                    })
                })
                .collect()
        }

        /// Whether no process with `tag` is left within two seconds.
        #[expect(
            clippy::disallowed_methods,
            reason = "the fake server's processes end on their own, and nothing reports it"
        )]
        fn all_gone(tag: &str) -> bool {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if processes_with(tag).is_empty() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        }

        fn child_pid(registry: &McpRegistry) -> u32 {
            let Transport::Stdio(stdio) = &registry.servers[0].transport else {
                unreachable!("a child process");
            };
            lock(&stdio.child).id()
        }

        #[test]
        fn a_sandboxed_server_reads_the_workspace_from_its_root_and_nothing_else() {
            let root = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            fs::write(root.path().join("notes.txt"), "plain notes\n").unwrap();
            fs::write(root.path().join(".env"), "TOKEN=fake-secret-value\n").unwrap();
            fs::write(home.path().join(".netrc"), "machine host password x\n").unwrap();
            fs::write(outside.path().join("canary"), "outside\n").unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let secret = "probe-secret-value-1234";
            let (registry, warnings) = connect_sandboxed(
                root.path(),
                home.path(),
                &[sandboxed_settings("box", &[], &[("PROBE_SECRET", secret)])],
            );

            let statuses = registry.statuses();
            assert!(
                matches!(statuses[0].state, McpServerState::Connected { .. }),
                "{statuses:?} {warnings:?}"
            );
            assert_eq!(
                statuses[0].transport,
                McpServerTransport::Local {
                    program: "python3".into(),
                    sandboxed: true,
                }
            );
            let target = registry.target("mcp__box__read").unwrap();
            assert!(target.sandboxed && !target.allowed);
            let workspace = root.path().canonicalize().unwrap();
            let place = call(&registry, "mcp__box__where", json!({})).unwrap();
            let (cwd, path) = place.split_once('\n').unwrap();
            assert_eq!(Path::new(cwd), workspace);
            assert!(
                std::env::split_paths(path).all(|entry| !entry.starts_with(&workspace)),
                "{path}"
            );
            let probe = |tool: &str, input: Value| call(&registry, tool, input);
            assert_eq!(
                probe("mcp__box__read", json!({"path": "notes.txt"})).unwrap(),
                "plain notes\n"
            );
            // A sensitive file is there, but /dev/null stands in for it,
            // which the sandbox's nodev mounts refuse to open.
            match probe("mcp__box__read", json!({"path": ".env"})) {
                Ok(text) => assert_eq!(text, ""),
                Err(err) => assert!(err.starts_with("PermissionError"), "{err}"),
            }
            for path in ["new.txt", "notes.txt", ".socium/scratch/x"] {
                let err = probe("mcp__box__write", json!({"path": path})).unwrap_err();
                assert!(err.contains("Error"), "{path}: {err}");
            }
            assert!(!root.path().join("new.txt").exists());
            assert_eq!(
                fs::read_to_string(root.path().join("notes.txt")).unwrap(),
                "plain notes\n"
            );
            let refused = probe(
                "mcp__box__connect",
                json!({"host": "127.0.0.1", "port": port}),
            )
            .unwrap_err();
            assert!(refused.contains("PermissionError"), "{refused}");
            assert_eq!(
                probe("mcp__box__getenv", json!({"name": "HOME"})).unwrap(),
                "/socium-home"
            );
            for path in [home.path().join(".netrc"), outside.path().join("canary")] {
                let err = probe(
                    "mcp__box__read",
                    json!({"path": path.display().to_string()}),
                )
                .unwrap_err();
                assert!(err.contains("FileNotFoundError"), "{err}");
            }
            // Its env reaches it, and nothing of the host's environment.
            assert_eq!(
                probe("mcp__box__getenv", json!({"name": "PROBE_SECRET"})).unwrap(),
                secret
            );
            for name in ["USER", "ANTHROPIC_API_KEY", "LD_PRELOAD"] {
                assert_eq!(
                    probe("mcp__box__getenv", json!({"name": name})).unwrap(),
                    "unset",
                    "{name}"
                );
            }
            // The child Socium started is bubblewrap, which runs on the
            // host: neither its environment nor its command line, which
            // any user can read, holds the value.
            let pid = child_pid(&registry);
            let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap();
            let environ = fs::read(format!("/proc/{pid}/environ")).unwrap();
            assert!(String::from_utf8_lossy(&cmdline).contains("bwrap"));
            for (what, bytes) in [("cmdline", &cmdline), ("environ", &environ)] {
                let text = String::from_utf8_lossy(bytes);
                assert!(!text.contains(secret), "{what}: {text}");
                assert!(!text.contains("PROBE_SECRET"), "{what}: {text}");
            }
        }

        /// The fail-closed path of ADR 33: no listing, no server; never the
        /// host, never asking instead.
        #[test]
        fn a_sandboxed_server_whose_secrets_cannot_be_hidden_is_not_started() {
            let root = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            for n in 0..257 {
                fs::write(root.path().join(format!(".env.{n}")), "TOKEN=x\n").unwrap();
            }
            let (registry, warnings) = connect_sandboxed(
                root.path(),
                home.path(),
                &[sandboxed_settings("box", &[], &[])],
            );

            assert!(registry.definitions().is_empty());
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(
                warnings[0].starts_with("MCP server box was not started: the files matching approval.sensitive could not be hidden from it")
                    && warnings[0].contains("more than 256"),
                "{warnings:?}"
            );
            assert!(matches!(
                &registry.statuses()[0].state,
                McpServerState::Failed { reason } if reason.contains("never runs with them in view")
            ));
        }

        #[test]
        fn a_sandboxed_server_runs_a_program_from_its_read_only_directories_only() {
            let root = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let tools = tempfile::tempdir().unwrap();
            let data = tempfile::tempdir().unwrap();
            fs::write(data.path().join("rules.yml"), "rules\n").unwrap();
            let program = tools.path().join("fake-mcp");
            fs::write(&program, fake_server_script()).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            let mount = |dir: &Path| ReadOnlyMount {
                source: dir.canonicalize().unwrap(),
                alias: None,
            };
            let mut server = sandboxed_settings("box", &[], &[]);
            server.transport = McpTransportSettings::Stdio {
                command: vec![program.display().to_string(), "--probe".into()],
                env: BTreeMap::new(),
                sandbox: Some(McpSandboxSettings {
                    read_only: vec![mount(tools.path()), mount(data.path())],
                }),
            };
            let (registry, warnings) = connect_sandboxed(root.path(), home.path(), &[server]);
            assert!(
                matches!(
                    registry.statuses()[0].state,
                    McpServerState::Connected { .. }
                ),
                "{warnings:?}"
            );
            let rules = data.path().canonicalize().unwrap().join("rules.yml");
            assert_eq!(
                call(
                    &registry,
                    "mcp__box__read",
                    json!({"path": rules.display().to_string()})
                )
                .unwrap(),
                "rules\n"
            );
            drop(registry);

            // A program in the workspace is one the model could write.
            fs::copy(&program, root.path().join("fake-mcp")).unwrap();
            let mut server = sandboxed_settings("box", &[], &[]);
            let McpTransportSettings::Stdio { command, .. } = &mut server.transport else {
                unreachable!()
            };
            *command = vec![
                root.path().join("fake-mcp").display().to_string(),
                "--probe".into(),
            ];
            let (registry, warnings) = connect_sandboxed(root.path(), home.path(), &[server]);
            assert!(registry.definitions().is_empty());
            assert!(
                warnings[0].contains("is in neither a system runtime"),
                "{warnings:?}"
            );
        }

        /// Checked again at start, when the workspace is known: settings
        /// cannot know it.
        #[test]
        fn a_read_only_directory_that_overlaps_the_workspace_is_refused_at_start() {
            let parent = tempfile::tempdir().unwrap();
            let root = parent.path().join("project");
            let home = tempfile::tempdir().unwrap();
            fs::create_dir(&root).unwrap();
            for source in [parent.path().to_path_buf(), root.join("src")] {
                fs::create_dir_all(&source).unwrap();
                let mut server = sandboxed_settings("box", &[], &[]);
                let McpTransportSettings::Stdio { sandbox, .. } = &mut server.transport else {
                    unreachable!()
                };
                sandbox.as_mut().unwrap().read_only.push(ReadOnlyMount {
                    source: source.canonicalize().unwrap(),
                    alias: None,
                });
                let (registry, warnings) = connect_sandboxed(&root, home.path(), &[server]);
                assert!(registry.definitions().is_empty());
                assert!(
                    warnings[0].contains("overlaps the workspace"),
                    "{warnings:?}"
                );
            }
        }

        /// Closing its input does not stop it, and neither does SIGTERM
        /// with `--linger=term`; dropping the registry still ends it, and
        /// the grandchild it detached into a session of its own, without
        /// hanging.
        #[test]
        #[expect(
            clippy::disallowed_methods,
            reason = "waits on purpose: a server ended with the thread that started it would be gone by then"
        )]
        fn dropping_the_registry_ends_a_sandboxed_server_that_ignores_its_input_and_its_detached_grandchild()
         {
            for linger in ["--linger", "--linger=term"] {
                let root = tempfile::tempdir().unwrap();
                let home = tempfile::tempdir().unwrap();
                let (server_tag, grandchild_tag) = (unique_tag("server"), unique_tag("grandchild"));
                let (registry, _) = connect_sandboxed(
                    root.path(),
                    home.path(),
                    &[sandboxed_settings(
                        "box",
                        &[linger, &format!("--tag={server_tag}")],
                        &[],
                    )],
                );
                // Still answering after the thread that started it ended.
                std::thread::sleep(Duration::from_millis(200));
                call(
                    &registry,
                    "mcp__box__detach",
                    json!({"tag": grandchild_tag}),
                )
                .unwrap();
                // bubblewrap's two processes carry the server's command
                // line too.
                assert!(!processes_with(&server_tag).is_empty(), "{linger}");
                assert_eq!(processes_with(&grandchild_tag).len(), 1, "{linger}");
                let bwrap = child_pid(&registry);

                let started = Instant::now();
                drop(registry);
                let took = started.elapsed();

                assert!(took >= STOP_GRACE, "{linger}: closing the input ended it");
                // SIGTERM ends bubblewrap, whose sandbox goes with it even
                // when the server ignores the signal, so the second grace
                // before SIGKILL is not waited out.
                assert!(took < STOP_GRACE * 2, "{linger}: {took:?}");
                assert!(all_gone(&server_tag), "{linger}: the server is left");
                assert!(
                    all_gone(&grandchild_tag),
                    "{linger}: the grandchild is left"
                );
                assert!(!Path::new(&format!("/proc/{bwrap}")).exists(), "{linger}");
            }
        }

        /// Closing its input is the first step of stopping, and a server
        /// that exits then ends the sandbox on its own: bubblewrap reports
        /// its exit code, not a signal.
        #[test]
        fn a_sandboxed_server_that_exits_when_its_input_closes_needs_no_signal() {
            let root = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let tag = unique_tag("graceful");
            let (registry, warnings) = connect_sandboxed(
                root.path(),
                home.path(),
                &[sandboxed_settings("box", &[&format!("--tag={tag}")], &[])],
            );
            assert!(!processes_with(&tag).is_empty(), "{warnings:?}");
            let Transport::Stdio(stdio) = &registry.servers[0].transport else {
                unreachable!("a child process");
            };

            stdio.close_input();

            // Generous, since only how it ends matters here.
            let status = stdio
                .wait_exited(Duration::from_secs(20))
                .expect("it did not exit");
            assert_eq!(status.code(), Some(0), "{status}");
            drop(registry);
            assert!(all_gone(&tag));
        }

        /// `mount` added to the server's `read_only`.
        fn with_mount(mut server: McpServerSettings, mount: ReadOnlyMount) -> McpServerSettings {
            let McpTransportSettings::Stdio { sandbox, .. } = &mut server.transport else {
                unreachable!()
            };
            sandbox.as_mut().unwrap().read_only.push(mount);
            server
        }

        /// The `read_only` check holds at start, not only when the
        /// settings were read: a directory, or one of its parents, swapped
        /// since for a link to the workspace, Socium's configuration or the
        /// MCP tokens is refused, and nothing is mounted through it.
        #[test]
        fn a_read_only_directory_swapped_for_a_link_since_the_settings_were_read_is_refused() {
            use std::os::unix::fs::symlink;
            let root = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let env = only_home(home.path());
            let config = crate::host::config_dir(&env).unwrap();
            let tokens = TokenStore::from_env(&env).unwrap().dir().to_path_buf();
            for dir in [&config, &tokens] {
                fs::create_dir_all(dir).unwrap();
            }
            let outer = tempfile::tempdir().unwrap();
            let outer = outer.path().canonicalize().unwrap();
            let mut case = 0;
            for guarded in [root.path(), &config, &tokens] {
                let guarded = guarded.canonicalize().unwrap();
                for parent_swapped in [false, true] {
                    case += 1;
                    let parent = outer.join(format!("{case}/parent"));
                    let source = parent.join(guarded.file_name().unwrap());
                    fs::create_dir_all(&source).unwrap();
                    // As the settings found it.
                    let mount = ReadOnlyMount {
                        source: source.clone(),
                        alias: None,
                    };
                    let server = with_mount(sandboxed_settings("box", &[], &[]), mount);
                    if parent_swapped {
                        fs::rename(&parent, outer.join(format!("{case}/before"))).unwrap();
                        symlink(guarded.parent().unwrap(), &parent).unwrap();
                    } else {
                        fs::remove_dir(&source).unwrap();
                        symlink(&guarded, &source).unwrap();
                    }

                    let (registry, warnings) =
                        connect_sandboxed(root.path(), home.path(), &[server]);

                    assert!(registry.definitions().is_empty(), "{case}: {warnings:?}");
                    assert!(
                        warnings[0].contains(&format!("now leads to {}", guarded.display())),
                        "{case}: {warnings:?}"
                    );
                }
            }
        }

        /// A secret stays hidden by every other name the server could open
        /// it by: a hard link, a path a pattern matches through a directory
        /// symlink, a `.socium` below the root, and the scratch directory.
        #[test]
        fn a_sandboxed_server_reads_no_secret_by_another_name() {
            use std::os::unix::fs::symlink;
            let root = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let secret = "TOKEN=by-another-name\n";
            for file in [".env", "real/secret.txt", "packages/app/.socium/.env"] {
                let path = root.path().join(file);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, secret).unwrap();
            }
            fs::write(root.path().join("real/readme.md"), "plain\n").unwrap();
            fs::hard_link(root.path().join(".env"), root.path().join("notes.txt")).unwrap();
            symlink("real", root.path().join("alias")).unwrap();
            let scratch = crate::state::scratch_dir(root.path()).unwrap();
            fs::write(scratch.join(".env"), secret).unwrap();
            let mut sensitive = crate::settings::DEFAULT_SENSITIVE.to_vec();
            sensitive.push("alias/secret.txt");
            let (registry, warnings) = McpRegistry::connect(
                &[sandboxed_settings("box", &[], &[])],
                root.path(),
                &PathPatterns::new(&sensitive),
                &only_home(home.path()),
                None,
                STARTUP_TIMEOUT,
                &TaskControl::default(),
            );
            assert!(
                matches!(
                    registry.statuses()[0].state,
                    McpServerState::Connected { .. }
                ),
                "{warnings:?}"
            );
            let read = |path: &str| call(&registry, "mcp__box__read", json!({"path": path}));

            assert_eq!(read("alias/readme.md").unwrap(), "plain\n");
            for path in [
                ".env",
                "notes.txt",
                "alias/secret.txt",
                "real/secret.txt",
                "packages/app/.socium/.env",
                ".socium/scratch/.env",
            ] {
                match read(path) {
                    Ok(text) => assert_eq!(text, "", "{path}"),
                    Err(err) => assert!(err.starts_with("PermissionError"), "{path}: {err}"),
                }
            }
        }

        /// A FIFO in a linked worktree's Git metadata fails the start at
        /// once, and the registry it leaves drops at once.
        #[test]
        fn a_fifo_in_worktree_metadata_fails_a_sandboxed_start_at_once() {
            for file in ["gitdir", "commondir"] {
                let parent = tempfile::tempdir().unwrap();
                let main = parent.path().join("main");
                fs::create_dir(&main).unwrap();
                crate::test_support::init_git_repo(&main);
                let linked = parent.path().join("linked");
                crate::test_support::run_git(
                    &main,
                    &["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
                );
                let metadata = main.join(".git/worktrees/linked").join(file);
                fs::remove_file(&metadata).unwrap();
                crate::test_support::make_fifo(&metadata);
                let home = tempfile::tempdir().unwrap();
                let home = home.path().to_path_buf();
                let (sent, received) = mpsc::channel();
                std::thread::spawn(move || {
                    let started = Instant::now();
                    let (registry, warnings) =
                        connect_sandboxed(&linked, &home, &[sandboxed_settings("box", &[], &[])]);
                    drop(registry);
                    let _ = sent.send((warnings, started.elapsed()));
                });

                let (warnings, took) = received
                    .recv_timeout(Duration::from_secs(30))
                    .unwrap_or_else(|_| panic!("{file}: the start waited on the FIFO"));

                assert!(
                    warnings[0].contains("linked worktree"),
                    "{file}: {warnings:?}"
                );
                assert!(took < Duration::from_secs(10), "{file}: {took:?}");
            }
        }

        /// A name not found on the sandbox's PATH says so, without the
        /// advice for commands to use `./`, which a server may not; a link
        /// in a read_only directory to a program elsewhere on the host is
        /// refused like one into the workspace.
        #[test]
        fn a_sandboxed_server_program_is_found_only_where_the_sandbox_mounts_it() {
            use std::os::unix::fs::{PermissionsExt, symlink};
            let root = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let tools = tempfile::tempdir().unwrap();
            let elsewhere = tempfile::tempdir().unwrap();
            let program = elsewhere.path().join("fake-mcp");
            fs::write(&program, fake_server_script()).unwrap();
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            fs::copy(&program, root.path().join("fake-mcp")).unwrap();
            let tools_dir = tools.path().canonicalize().unwrap();
            for (link, target) in [
                ("elsewhere", program.clone()),
                ("into-workspace", root.path().join("fake-mcp")),
            ] {
                symlink(&target, tools_dir.join(link)).unwrap();
            }
            let server = |command: Vec<String>| {
                let mut server = sandboxed_settings("box", &[], &[]);
                server.transport = McpTransportSettings::Stdio {
                    command,
                    env: BTreeMap::new(),
                    sandbox: Some(McpSandboxSettings {
                        read_only: vec![ReadOnlyMount {
                            source: tools_dir.clone(),
                            alias: None,
                        }],
                    }),
                };
                server
            };
            for link in ["elsewhere", "into-workspace"] {
                let command = vec![tools_dir.join(link).display().to_string(), "--probe".into()];
                let (registry, warnings) =
                    connect_sandboxed(root.path(), home.path(), &[server(command)]);
                assert!(registry.definitions().is_empty(), "{link}");
                assert!(
                    warnings[0].contains("is in neither a system runtime"),
                    "{link}: {warnings:?}"
                );
            }

            let missing = vec!["socium-no-such-mcp-server".to_string()];
            let (registry, warnings) =
                connect_sandboxed(root.path(), home.path(), &[server(missing)]);

            assert!(registry.definitions().is_empty());
            assert!(
                warnings[0].contains("not found in system runtime PATH: socium-no-such-mcp-server")
                    && !warnings[0].contains("./"),
                "{warnings:?}"
            );
        }

        /// A start cancelled while the sensitive files are listed stops
        /// there, and the server never runs.
        #[test]
        fn a_sandboxed_start_cancelled_while_listing_never_runs_the_server() {
            let root = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            for n in 0..20 {
                fs::write(root.path().join(format!("f{n}")), "x").unwrap();
            }
            let tag = unique_tag("cancelled");
            let stop = TaskControl::default();
            stop.cancel();

            let (registry, warnings) = McpRegistry::connect(
                &[sandboxed_settings("box", &[&format!("--tag={tag}")], &[])],
                root.path(),
                &PathPatterns::new(crate::settings::DEFAULT_SENSITIVE),
                &only_home(home.path()),
                None,
                STARTUP_TIMEOUT,
                &stop,
            );

            assert!(registry.definitions().is_empty());
            assert!(
                warnings[0].contains("the listing of protected files was cancelled"),
                "{warnings:?}"
            );
            assert!(processes_with(&tag).is_empty());
        }

        /// ADR 33 fails closed: a workspace directory the listing cannot
        /// read may hold a secret, so the server is not started.
        #[test]
        fn a_sandboxed_server_is_not_started_when_a_workspace_directory_cannot_be_read() {
            use std::os::unix::fs::PermissionsExt;
            /// Gives the directory back its access, so it can be removed.
            struct Unlock(std::path::PathBuf);
            impl Drop for Unlock {
                fn drop(&mut self) {
                    let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
                }
            }
            let root = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let locked = root.path().join("locked");
            fs::create_dir(&locked).unwrap();
            fs::write(locked.join(".env"), "TOKEN=x\n").unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
            let _unlock = Unlock(locked);

            let (registry, warnings) = connect_sandboxed(
                root.path(),
                home.path(),
                &[sandboxed_settings("box", &[], &[])],
            );

            assert!(registry.definitions().is_empty());
            assert!(
                warnings[0].starts_with("MCP server box was not started: the files matching approval.sensitive could not be hidden from it")
                    && warnings[0].contains("could not be listed"),
                "{warnings:?}"
            );
        }
    }
}
