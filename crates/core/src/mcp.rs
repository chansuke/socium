//! A client for MCP servers the user configured in `config.toml` (ADR 20):
//! each server runs on the host as a child process that speaks
//! newline-delimited JSON-RPC on stdin and stdout, and its tools are offered
//! to the main agent as `mcp__<server>__<tool>`.
//!
//! Only tools are used. A server starts in `/`, finds its program on a PATH
//! without the workspace in it, and gets a fixed environment (PATH, HOME,
//! USER, LANG and what its table sets), so the workspace cannot choose the
//! code that runs and provider keys never reach it.

use crate::TaskControl;
use crate::settings::McpServerSettings;
use crate::tools::ToolDefinition;
use crate::tools::error::ToolError;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

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
const PREFIX: &str = "mcp__";
/// The longest tool name model providers accept.
const MAX_TOOL_NAME: usize = 64;
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
/// Messages waiting for the server to read its input; a sender waits while
/// the queue is full.
const OUTBOX_MESSAGES: usize = 64;
/// How long a stopping server gets after its input closes, and again after
/// SIGTERM, before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(1);
/// Variable values shorter than this are not treated as secrets, so a value
/// such as `1` does not blank out every digit of a warning.
const MIN_SECRET_CHARS: usize = 8;
/// How often a waiting call checks for cancellation.
const POLL: Duration = Duration::from_millis(50);

/// Whether `name` has the form of an MCP tool's exposed name.
pub(crate) fn is_mcp_tool(name: &str) -> bool {
    name.starts_with(PREFIX)
}

type Waiters = Arc<Mutex<Option<HashMap<u64, mpsc::Sender<Value>>>>>;
/// Lines for the writer thread; `None` once the server's input is closed.
type Outbox = Arc<Mutex<Option<mpsc::SyncSender<Vec<u8>>>>>;

/// One running server. It is stopped when this is dropped.
pub(crate) struct McpServer {
    name: String,
    child: Mutex<Child>,
    outbox: Outbox,
    /// Callers waiting for a response, by request ID; `None` once the
    /// server's output closed.
    waiters: Waiters,
    next_id: AtomicU64,
    stderr: Arc<Mutex<VecDeque<String>>>,
    secrets: Secrets,
    call_timeout: Duration,
}

#[derive(Debug)]
enum RequestError {
    /// The server exited or closed its output.
    Gone,
    Cancelled,
    TimedOut,
    /// A JSON-RPC error response.
    Refused(String),
    /// A response that is neither a result nor an error.
    Malformed,
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
struct Secrets(Vec<String>);

impl Secrets {
    fn of(settings: &McpServerSettings) -> Self {
        let mut values = settings
            .env
            .values()
            .filter(|value| value.chars().count() >= MIN_SECRET_CHARS)
            .cloned()
            .collect::<Vec<_>>();
        // Longest first, so a value that contains another is replaced whole.
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        Self(values)
    }

    fn redact(&self, text: &str) -> String {
        self.0.iter().fold(text.to_string(), |text, value| {
            text.replace(value, "[redacted]")
        })
    }
}

/// A tool as the server listed it.
struct RemoteTool {
    name: String,
    description: String,
    input_schema: Value,
}

impl McpServer {
    /// Starts the server, performs the handshake, and lists its tools,
    /// within `timeout` and until `stop` is cancelled. `Err` is a warning
    /// for the user, with the end of the server's standard error.
    fn connect(
        settings: &McpServerSettings,
        workspace_root: &Path,
        host_env: &dyn Fn(&str) -> Option<String>,
        timeout: Duration,
        stop: &TaskControl,
    ) -> Result<(Self, Vec<RemoteTool>), String> {
        let wait = Wait {
            deadline: Instant::now() + timeout,
            control: stop,
        };
        let server = Self::spawn(settings, workspace_root, host_env)?;
        let failed = |server: &Self, what: &str, err: RequestError| {
            let reason = match err {
                RequestError::TimedOut => {
                    format!("did not {what} within {} seconds", timeout.as_secs())
                }
                RequestError::Refused(message) => format!("refused to {what}: {message}"),
                RequestError::Malformed => format!("sent a malformed answer when asked to {what}"),
                RequestError::Gone | RequestError::Cancelled => {
                    format!("stopped before it could {what}{}", server.exit_status())
                }
            };
            server.secrets.redact(&format!(
                "MCP server {} was not started: it {reason}.{}",
                server.name,
                server.stderr_tail()
            ))
        };
        let refuse = |server: &Self, problem: String| {
            server.secrets.redact(&format!(
                "MCP server {} was not started: {problem}.",
                server.name
            ))
        };
        let initialized = server
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "socium", "version": env!("CARGO_PKG_VERSION")},
                }),
                &wait,
            )
            .map_err(|err| failed(&server, "initialize", err))?;
        let version = initialized["protocolVersion"].as_str().unwrap_or_default();
        if !PROTOCOL_VERSIONS.contains(&version) {
            return Err(refuse(
                &server,
                format!(
                    "it answered with protocol version `{version}`, and Socium speaks {}",
                    PROTOCOL_VERSIONS.join(", ")
                ),
            ));
        }
        server
            .notify("notifications/initialized", None, &wait)
            .map_err(|err| failed(&server, "initialize", err))?;
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
                return Err(refuse(&server, "its tool list is not an array".into()));
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
        Err(refuse(
            &server,
            format!("its tool list has more than {MAX_TOOL_PAGES} pages"),
        ))
    }

    fn spawn(
        settings: &McpServerSettings,
        workspace_root: &Path,
        host_env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let mut command = Command::new(&settings.command[0]);
        // Not the workspace: a launcher such as `python3 -m` or `npx` would
        // load the workspace's modules and packages first.
        command
            .args(&settings.command[1..])
            .current_dir("/")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for variable in INHERITED_VARIABLES {
            let value = match *variable {
                "PATH" => host_env(variable).map(|path| path_outside(&path, workspace_root)),
                _ => host_env(variable),
            };
            if let Some(value) = value {
                command.env(variable, value);
            }
        }
        command.envs(&settings.env);
        // Its own process group, so a launcher such as npx or uvx goes down
        // with the server it started.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().map_err(|err| {
            format!(
                "MCP server {} was not started: `{}` could not run: {err}.",
                settings.name, settings.command[0]
            )
        })?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            unreachable!("all three streams are piped");
        };
        let (sender, lines) = mpsc::sync_channel(OUTBOX_MESSAGES);
        let server = Self {
            name: settings.name.clone(),
            child: Mutex::new(child),
            outbox: Arc::new(Mutex::new(Some(sender))),
            waiters: Arc::new(Mutex::new(Some(HashMap::new()))),
            next_id: AtomicU64::new(1),
            stderr: Arc::new(Mutex::new(VecDeque::new())),
            secrets: Secrets::of(settings),
            call_timeout: settings.tool_timeout,
        };
        std::thread::spawn(move || write_messages(stdin, &lines));
        let (waiters, outbox) = (server.waiters.clone(), server.outbox.clone());
        std::thread::spawn(move || read_messages(stdout, &waiters, &outbox));
        let kept = server.stderr.clone();
        std::thread::spawn(move || keep_stderr_tail(stderr, &kept));
        Ok(server)
    }

    fn request(&self, method: &str, params: Value, wait: &Wait<'_>) -> Result<Value, RequestError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, responses) = mpsc::channel();
        match lock(&self.waiters).as_mut() {
            Some(waiters) => waiters.insert(id, sender),
            None => return Err(RequestError::Gone),
        };
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let response = self.send(&message, Some(wait)).and_then(|()| {
            loop {
                match responses.recv_timeout(POLL) {
                    Ok(response) => break Ok(response),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break Err(RequestError::Gone),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if let Some(stop) = wait.stopped() {
                    break Err(stop);
                }
            }
        });
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                self.forget(id);
                // A client must not cancel its initialize request; the
                // server is stopped instead.
                if matches!(err, RequestError::Cancelled | RequestError::TimedOut)
                    && method != "initialize"
                {
                    let reason = match err {
                        RequestError::Cancelled => "The user cancelled the request.",
                        _ => "The request took too long.",
                    };
                    let notice = json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/cancelled",
                        "params": {"requestId": id, "reason": reason},
                    });
                    let _ = self.send(&notice, None);
                }
                return Err(err);
            }
        };
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

    fn forget(&self, id: u64) {
        if let Some(waiters) = lock(&self.waiters).as_mut() {
            waiters.remove(&id);
        }
    }

    fn notify(
        &self,
        method: &str,
        params: Option<Value>,
        wait: &Wait<'_>,
    ) -> Result<(), RequestError> {
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.send(&message, Some(wait))
    }

    /// Queues `message` for the server's input. While the queue is full it
    /// waits until `wait` ends, or gives up at once without one.
    fn send(&self, message: &Value, wait: Option<&Wait<'_>>) -> Result<(), RequestError> {
        let mut line = message.to_string().into_bytes();
        line.push(b'\n');
        loop {
            let queued = match lock(&self.outbox).as_ref() {
                Some(outbox) => outbox.try_send(line),
                None => return Err(RequestError::Gone),
            };
            match queued {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Disconnected(_)) => return Err(RequestError::Gone),
                Err(mpsc::TrySendError::Full(unsent)) => line = unsent,
            }
            match wait {
                None => return Err(RequestError::Gone),
                Some(wait) => {
                    if let Some(stop) = wait.stopped() {
                        return Err(stop);
                    }
                }
            }
            std::thread::sleep(POLL);
        }
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
                ToolError::Message(self.secrets.redact(&match err {
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
                    // Its standard error stays out: it is for the user, and
                    // may hold what the server was given.
                    RequestError::Gone => format!(
                        "The {} MCP server stopped before answering{}",
                        self.name,
                        self.exit_status()
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
            return Err(ToolError::Message(text));
        }
        Ok(text)
    }

    /// `" (exit status: 3)"` when the child has exited, else nothing.
    fn exit_status(&self) -> String {
        // The reader may see the pipe close a moment before the exit is
        // visible.
        for _ in 0..10 {
            if let Some(status) = self.exited() {
                return format!(" ({status})");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        String::new()
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

    /// Closes the server's input, which tells it to exit.
    fn close_input(&self) {
        lock(&self.outbox).take();
    }

    /// Whether the child exits within `grace`.
    fn exits_within(&self, grace: Duration) -> bool {
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if self.exited().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// The last lines the server wrote to standard error, as a paragraph
    /// to append to a warning, or nothing.
    fn stderr_tail(&self) -> String {
        // Lines written just before an exit may still be on their way.
        std::thread::sleep(Duration::from_millis(20));
        let lines = lock(&self.stderr);
        if lines.is_empty() {
            return String::new();
        }
        format!(
            " Its last standard error lines:\n{}",
            lines.iter().cloned().collect::<Vec<_>>().join("\n")
        )
    }
}

impl Drop for McpServer {
    /// Closes the input and gives the server a moment to exit, then sends
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
        let mut child = lock(&self.child);
        let _ = child.kill();
        let _ = child.wait();
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

/// Writes queued lines to the server's input until the queue closes or the
/// server stops reading. A blocked write holds no lock, so cancelling and
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
                let _ = waiter.send(Value::Object(message));
            }
        }
        (None, None) => {}
    }
}

/// Keeps the last lines of standard error, reading at most a line's worth
/// of each line however long it runs.
fn keep_stderr_tail(stderr: impl Read, lines: &Mutex<VecDeque<String>>) {
    let line_bytes = STDERR_LINE_CHARS * 4;
    let keep = |line: &[u8]| {
        let line = String::from_utf8_lossy(line);
        let line = line.trim_end();
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
    loop {
        let (used, ended) = match reader.fill_buf() {
            Ok([]) | Err(_) => break,
            Ok(chunk) => {
                let end = chunk.iter().position(|byte| *byte == b'\n');
                let part = &chunk[..end.unwrap_or(chunk.len())];
                let room = line_bytes.saturating_sub(line.len());
                line.extend_from_slice(&part[..part.len().min(room)]);
                (end.map_or(chunk.len(), |end| end + 1), end.is_some())
            }
        };
        reader.consume(used);
        if ended {
            keep(&line);
            line.clear();
        }
    }
    keep(&line);
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
}

struct Route {
    server: usize,
    remote: String,
    allowed: bool,
}

/// The servers that started and the tools they offer.
#[derive(Default)]
pub(crate) struct McpRegistry {
    servers: Vec<McpServer>,
    routes: HashMap<String, Route>,
    definitions: Vec<ToolDefinition>,
}

impl McpRegistry {
    /// Starts every server at once and waits for each up to `timeout` or
    /// until `stop` is cancelled. A server that fails is left out with a
    /// warning; so is a tool whose exposed name providers would refuse.
    pub(crate) fn connect(
        servers: &[McpServerSettings],
        workspace_root: &Path,
        host_env: &(dyn Fn(&str) -> Option<String> + Sync),
        timeout: Duration,
        stop: &TaskControl,
    ) -> (Self, Vec<String>) {
        let connected = std::thread::scope(|scope| {
            let handles = servers
                .iter()
                .map(|settings| {
                    scope.spawn(move || {
                        McpServer::connect(settings, workspace_root, host_env, timeout, stop)
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err("An MCP server's startup panicked.".into()))
                })
                .collect::<Vec<_>>()
        });
        let mut registry = Self::default();
        let mut warnings = Vec::new();
        let mut all_bytes = 0;
        for (settings, result) in servers.iter().zip(connected) {
            let (server, tools) = match result {
                Ok(connected) => connected,
                Err(warning) => {
                    warnings.push(warning);
                    continue;
                }
            };
            let index = registry.servers.len();
            let (mut offered, mut bytes, mut left_out) = (0, 0, 0);
            for tool in tools {
                let exposed = format!("{PREFIX}{}__{}", settings.name, tool.name);
                let valid = exposed.len() <= MAX_TOOL_NAME
                    && exposed
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
                if !valid {
                    warnings.push(format!(
                        "MCP server {}: tool `{}` is not offered, since `{exposed}` is not a tool name model providers accept (at most {MAX_TOOL_NAME} letters, digits, '_' or '-')",
                        settings.name, tool.name
                    ));
                    continue;
                }
                if registry.routes.contains_key(&exposed) {
                    warnings.push(format!(
                        "MCP server {}: tool `{}` is not offered, since another tool is already named `{exposed}`",
                        settings.name, tool.name
                    ));
                    continue;
                }
                if tool.input_schema.to_string().len() > MAX_SCHEMA_BYTES {
                    warnings.push(format!(
                        "MCP server {}: tool `{}` is not offered, since its input schema is larger than {} KiB",
                        settings.name,
                        tool.name,
                        MAX_SCHEMA_BYTES / 1024
                    ));
                    continue;
                }
                let size = tool.description.len() + tool.input_schema.to_string().len();
                if offered == MAX_TOOLS_PER_SERVER
                    || registry.definitions.len() == MAX_MCP_TOOLS
                    || bytes + size > MAX_SERVER_DEFINITION_BYTES
                    || all_bytes + size > MAX_MCP_DEFINITION_BYTES
                {
                    left_out += 1;
                    continue;
                }
                (offered, bytes, all_bytes) = (offered + 1, bytes + size, all_bytes + size);
                registry.definitions.push(ToolDefinition {
                    name: exposed.clone(),
                    description: tool.description,
                    input_schema: tool.input_schema,
                });
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
            registry.servers.push(server);
        }
        (registry, warnings)
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
            server.close_input();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

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
            command,
            env: BTreeMap::from([("FAKE_CONFIGURED".to_string(), "yes".to_string())]),
            allow: allow.iter().map(|tool| tool.to_string()).collect(),
            tool_timeout: Duration::from_secs(600),
        }
    }

    /// The parent's environment plus a provider key, as Socium would see
    /// them.
    pub(crate) fn host_env(name: &str) -> Option<String> {
        match name {
            "ANTHROPIC_API_KEY" => Some("sk-ant-must-not-leak".into()),
            other => std::env::var(other).ok(),
        }
    }

    pub(crate) fn connect_fake(servers: &[McpServerSettings]) -> (McpRegistry, Vec<String>) {
        let root = tempfile::tempdir().unwrap();
        connect_in(root.path(), servers, &host_env, STARTUP_TIMEOUT)
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
            command: vec!["/nonexistent/mcp-server".into()],
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

    #[cfg(target_os = "linux")]
    #[test]
    fn dropping_the_registry_ends_the_server() {
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &[])]);
        let pid = lock(&registry.servers[0].child).id();
        assert!(Path::new(&format!("/proc/{pid}")).exists());

        drop(registry);

        // Reaped, not left a zombie.
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }

    fn connect_in(
        root: &Path,
        servers: &[McpServerSettings],
        env: &(dyn Fn(&str) -> Option<String> + Sync),
        timeout: Duration,
    ) -> (McpRegistry, Vec<String>) {
        McpRegistry::connect(servers, root, env, timeout, &TaskControl::default())
    }

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
        let python = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join("python3"))
            .find(|path| path.is_file())
            .unwrap();
        let mut settings = fake_settings("fake", "", &[]);
        settings.command[0] = python.display().to_string();
        let (registry, warnings) = connect_in(root.path(), &[settings], &env, STARTUP_TIMEOUT);
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
        let settings = McpServerSettings {
            command: vec!["python3".into(), "-m".into(), "socium_planted".into()],
            ..fake_settings("planted", "", &[])
        };

        let (registry, warnings) =
            connect_in(root.path(), &[settings], &host_env, Duration::from_secs(5));

        assert!(registry.definitions().is_empty());
        assert!(warnings[0].contains("was not started"), "{warnings:?}");
        assert!(!marker.exists());
    }

    #[test]
    fn a_server_that_stops_reading_does_not_hold_up_cancelling_or_stopping() {
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &[])]);
        let pid = lock(&registry.servers[0].child).id();
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
            settings
                .env
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
        settings
            .env
            .insert("FAKE_LOG".into(), log.display().to_string());

        let (_, warnings) = connect_in(root.path(), &[settings], &host_env, Duration::from_secs(1));

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
            &host_env,
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

        keep_stderr_tail(stream.as_slice(), &lines);

        let lines = lines.into_inner().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "x".repeat(STDERR_LINE_CHARS));
        assert_eq!(lines[1], "last words");
    }
}
