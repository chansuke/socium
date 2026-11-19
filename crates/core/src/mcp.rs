//! A client for MCP servers the user configured in `config.toml` (ADR 20):
//! each server runs on the host as a child process that speaks
//! newline-delimited JSON-RPC on stdin and stdout, and its tools are offered
//! to the main agent as `mcp__<server>__<tool>`.
//!
//! Only tools are used. A server gets the workspace as its directory and a
//! fixed environment (PATH, HOME, USER, LANG and what its table sets), so
//! provider keys and other variables in Socium's environment never reach it.

use crate::TaskControl;
use crate::settings::McpServerSettings;
use crate::tools::ToolDefinition;
use crate::tools::error::ToolError;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// The revision this client implements. A server may answer with another
/// one; the tools methods are the same in every revision so far.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// The longest a server may take to start and list its tools.
pub(crate) const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Host variables a server gets when they are set; everything else in
/// Socium's environment stays out.
const INHERITED_VARIABLES: &[&str] = &["PATH", "HOME", "USER", "LANG"];
/// What exposed tool names begin with.
const PREFIX: &str = "mcp__";
/// The longest tool name model providers accept.
const MAX_TOOL_NAME: usize = 64;
/// Lines of a server's standard error kept for error messages, and the
/// longest kept line.
const STDERR_LINES: usize = 20;
const STDERR_LINE_CHARS: usize = 400;
/// A message larger than this ends the connection.
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// Pages of `tools/list` read before giving up on a server that never
/// stops paginating.
const MAX_TOOL_PAGES: usize = 100;
/// How often a waiting call checks for cancellation.
const POLL: Duration = Duration::from_millis(50);

/// Whether `name` has the form of an MCP tool's exposed name.
pub(crate) fn is_mcp_tool(name: &str) -> bool {
    name.starts_with(PREFIX)
}

type Waiters = Arc<Mutex<Option<HashMap<u64, mpsc::Sender<Value>>>>>;
type Writer = Arc<Mutex<Option<ChildStdin>>>;

/// One running server. The child is killed when this is dropped.
pub(crate) struct McpServer {
    name: String,
    child: Mutex<Child>,
    stdin: Writer,
    /// Callers waiting for a response, by request ID; `None` once the
    /// server's output closed.
    waiters: Waiters,
    next_id: AtomicU64,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

#[derive(Debug)]
enum RequestError {
    /// The server exited or closed its output.
    Gone,
    Cancelled,
    TimedOut,
    /// A JSON-RPC error response.
    Refused(String),
}

/// How long a request may wait for its response.
enum Wait<'a> {
    Until(Instant),
    /// Until the response arrives or the task is cancelled.
    Cancellable(&'a TaskControl),
}

/// A tool as the server listed it.
struct RemoteTool {
    name: String,
    description: String,
    input_schema: Value,
}

impl McpServer {
    /// Starts the server, performs the handshake, and lists its tools, all
    /// within `timeout`. `Err` is a warning for the user, with the end of
    /// the server's standard error.
    fn connect(
        settings: &McpServerSettings,
        workspace_root: &Path,
        host_env: &dyn Fn(&str) -> Option<String>,
        timeout: Duration,
    ) -> Result<(Self, Vec<RemoteTool>), String> {
        let deadline = Instant::now() + timeout;
        let server = Self::spawn(settings, workspace_root, host_env)?;
        let failed = |server: &Self, what: &str, err: RequestError| {
            let reason = match err {
                RequestError::TimedOut => {
                    format!("did not {what} within {} seconds", timeout.as_secs())
                }
                RequestError::Refused(message) => format!("refused to {what}: {message}"),
                RequestError::Gone | RequestError::Cancelled => {
                    format!("stopped before it could {what}{}", server.exit_status())
                }
            };
            format!(
                "MCP server {} was not started: it {reason}.{}",
                server.name,
                server.stderr_tail()
            )
        };
        let initialized = server
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "socium", "version": env!("CARGO_PKG_VERSION")},
                }),
                Wait::Until(deadline),
            )
            .map_err(|err| failed(&server, "initialize", err))?;
        if !initialized.is_object() {
            return Err(format!(
                "MCP server {} was not started: its initialize result is not an object.",
                server.name
            ));
        }
        server
            .notify("notifications/initialized", None)
            .map_err(|err| failed(&server, "initialize", err))?;
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_TOOL_PAGES {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let page = server
                .request("tools/list", params, Wait::Until(deadline))
                .map_err(|err| failed(&server, "list its tools", err))?;
            for tool in page["tools"].as_array().into_iter().flatten() {
                let Some(name) = tool["name"].as_str() else {
                    continue;
                };
                let description = tool["description"]
                    .as_str()
                    .or_else(|| tool["title"].as_str())
                    .filter(|text| !text.trim().is_empty())
                    .map(str::to_string)
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
        Err(format!(
            "MCP server {} was not started: its tool list has more than {MAX_TOOL_PAGES} pages.",
            server.name
        ))
    }

    fn spawn(
        settings: &McpServerSettings,
        workspace_root: &Path,
        host_env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let mut command = Command::new(&settings.command[0]);
        command
            .args(&settings.command[1..])
            .current_dir(workspace_root)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for variable in INHERITED_VARIABLES {
            if let Some(value) = host_env(variable) {
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
        let server = Self {
            name: settings.name.clone(),
            child: Mutex::new(child),
            stdin: Arc::new(Mutex::new(Some(stdin))),
            waiters: Arc::new(Mutex::new(Some(HashMap::new()))),
            next_id: AtomicU64::new(1),
            stderr: Arc::new(Mutex::new(VecDeque::new())),
        };
        let (waiters, writer) = (server.waiters.clone(), server.stdin.clone());
        std::thread::spawn(move || read_messages(stdout, &waiters, &writer));
        let lines = server.stderr.clone();
        std::thread::spawn(move || keep_stderr_tail(stderr, &lines));
        Ok(server)
    }

    fn request(&self, method: &str, params: Value, wait: Wait<'_>) -> Result<Value, RequestError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, responses) = mpsc::channel();
        match lock(&self.waiters).as_mut() {
            Some(waiters) => waiters.insert(id, sender),
            None => return Err(RequestError::Gone),
        };
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if write_message(&self.stdin, &message).is_err() {
            self.forget(id);
            return Err(RequestError::Gone);
        }
        loop {
            match responses.recv_timeout(POLL) {
                Ok(response) => {
                    if let Some(error) = response.get("error") {
                        let message = error["message"].as_str().unwrap_or("no message given");
                        return Err(RequestError::Refused(message.to_string()));
                    }
                    return Ok(response.get("result").cloned().unwrap_or(Value::Null));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(RequestError::Gone),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let stop = match wait {
                Wait::Until(deadline) => {
                    (Instant::now() >= deadline).then_some(RequestError::TimedOut)
                }
                Wait::Cancellable(control) => {
                    control.is_cancelled().then_some(RequestError::Cancelled)
                }
            };
            if let Some(stop) = stop {
                self.forget(id);
                let reason = match stop {
                    RequestError::Cancelled => "The user cancelled the request.",
                    _ => "The request took too long.",
                };
                let _ = self.notify(
                    "notifications/cancelled",
                    Some(json!({"requestId": id, "reason": reason})),
                );
                return Err(stop);
            }
        }
    }

    fn forget(&self, id: u64) {
        if let Some(waiters) = lock(&self.waiters).as_mut() {
            waiters.remove(&id);
        }
    }

    fn notify(&self, method: &str, params: Option<Value>) -> Result<(), RequestError> {
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        write_message(&self.stdin, &message).map_err(|_| RequestError::Gone)
    }

    /// Runs `tool` with `arguments` until it answers or the task is
    /// cancelled, and turns its content into the text the model reads.
    fn call(
        &self,
        tool: &str,
        arguments: &Value,
        control: &TaskControl,
    ) -> Result<String, ToolError> {
        let arguments = match arguments {
            Value::Null => json!({}),
            other => other.clone(),
        };
        let result = self
            .request(
                "tools/call",
                json!({"name": tool, "arguments": arguments}),
                Wait::Cancellable(control),
            )
            .map_err(|err| match err {
                RequestError::Cancelled => ToolError::Message(format!(
                    "Cancelled; the {} MCP server was asked to stop the call",
                    self.name
                )),
                RequestError::Refused(message) => ToolError::Message(format!(
                    "The {} MCP server refused the call: {message}",
                    self.name
                )),
                RequestError::Gone | RequestError::TimedOut => ToolError::Message(format!(
                    "The {} MCP server stopped before answering{}.{}",
                    self.name,
                    self.exit_status(),
                    self.stderr_tail()
                )),
            })?;
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
            if let Ok(Some(status)) = lock(&self.child).try_wait() {
                return format!(" ({status})");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        String::new()
    }

    /// The last lines the server wrote to standard error, as a paragraph
    /// to append to a message, or nothing.
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
    fn drop(&mut self) {
        lock(&self.stdin).take();
        let mut child = lock(&self.child);
        #[cfg(unix)]
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: kill only sends a signal; the group is the child's own.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn write_message(stdin: &Writer, message: &Value) -> std::io::Result<()> {
    let mut stdin = lock(stdin);
    let stdin = stdin.as_mut().ok_or(std::io::ErrorKind::BrokenPipe)?;
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    stdin.write_all(&line)?;
    stdin.flush()
}

/// Routes each response to the caller waiting for its ID and answers the
/// server's own requests until the server's output closes; then every
/// waiting caller is told the server is gone.
fn read_messages(stdout: impl Read, waiters: &Waiters, stdin: &Writer) {
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
        // Anything that is not a JSON object, such as a stray log line, is
        // skipped.
        let Ok(Value::Object(message)) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        match (message.get("method"), message.get("id")) {
            (Some(method), Some(id)) => {
                // The client offers no capabilities, so it answers only
                // pings.
                let reply = if method == "ping" {
                    json!({"jsonrpc": "2.0", "id": id, "result": {}})
                } else {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}})
                };
                let _ = write_message(stdin, &reply);
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
    lock(waiters).take();
}

fn keep_stderr_tail(stderr: impl Read, lines: &Mutex<VecDeque<String>>) {
    for line in BufReader::new(stderr).split(b'\n') {
        let Ok(line) = line else { break };
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut lines = lock(lines);
        if lines.len() == STDERR_LINES {
            lines.pop_front();
        }
        lines.push_back(line.chars().take(STDERR_LINE_CHARS).collect());
    }
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
    /// Starts every server at once and waits for each up to `timeout`. A
    /// server that fails is left out with a warning; so is a tool whose
    /// exposed name providers would refuse.
    pub(crate) fn connect(
        servers: &[McpServerSettings],
        workspace_root: &Path,
        host_env: &(dyn Fn(&str) -> Option<String> + Sync),
        timeout: Duration,
    ) -> (Self, Vec<String>) {
        let connected = std::thread::scope(|scope| {
            let handles = servers
                .iter()
                .map(|settings| {
                    scope.spawn(move || {
                        McpServer::connect(settings, workspace_root, host_env, timeout)
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
        for (settings, result) in servers.iter().zip(connected) {
            let (server, tools) = match result {
                Ok(connected) => connected,
                Err(warning) => {
                    warnings.push(warning);
                    continue;
                }
            };
            let index = registry.servers.len();
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
        McpRegistry::connect(servers, root.path(), &host_env, STARTUP_TIMEOUT)
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
    fn a_server_that_exits_during_a_call_is_reported_with_its_stderr() {
        let (registry, _) = connect_fake(&[fake_settings("fake", "", &[])]);

        let err = call(&registry, "mcp__fake__die", json!({})).unwrap_err();

        assert!(err.contains("The fake MCP server stopped"), "{err}");
        assert!(err.contains("exit status: 3"), "{err}");
        assert!(err.contains("fatal: the database is gone"), "{err}");
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
        let (registry, warnings) = McpRegistry::connect(
            &[
                fake_settings("crash", "--crash", &[]),
                fake_settings("hang", "--hang", &[]),
                missing,
                fake_settings("good", "", &[]),
            ],
            root.path(),
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
}
