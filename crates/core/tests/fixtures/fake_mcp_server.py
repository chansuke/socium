"""A small MCP server over stdio or Streamable HTTP for Socium's client tests.

`--crash` exits during startup and `--hang` never answers `initialize`.
`--version=V` answers `initialize` with revision V, `--no-tools` offers no
tools capability, `--big` adds a tool with a long description and one with
a huge schema, `--many` adds 80 small tools, `--heavy` adds four with
schemas of about 50 KB, and `--on-eof=PATH` writes PATH when its input closes. With
`FAKE_LOG` set it appends each method it receives to that file.
Otherwise it lists its tools over two pages and answers `tools/call`:
`echo`, `fail` (isError), `image`, `structured`, `slow` (never answers,
so the caller must cancel), `cancelled` (the request IDs the client
cancelled), `die` (exits mid-call), `env` (the variable names it sees),
`pong` (whether the client answered its ping), `where` (its directory and
PATH), `deaf` (stops reading its input), `malformed` (an answer with
neither result nor error), `empty` (a result without content), `batch`
(its answer inside a batch), `secret` (an error that repeats
`FAKE_SECRET`) and `expire` (over HTTP, ends every session).

`--http` serves the MCP endpoint `/mcp` on 127.0.0.1 and prints
`PORT <n>` once it listens. With `--sse` every request is answered with an
event stream that carries a notification first (and a ping for `pong`);
otherwise with one JSON body. `--cr` starts each stream with a BOM and ends
its lines with CR alone. Over HTTP, `reflect` answers HTTP 400 with the
Authorization header in its error. `--token=T` requires `Authorization: Bearer
T`. `--log=PATH` appends one JSON line per HTTP request: the HTTP method,
the JSON-RPC method, and the session and protocol version headers.
"""

import json
import os
import sys
import threading
import time

MODE = sys.argv[1] if len(sys.argv) > 1 else ""
OPTIONS = dict(arg[2:].partition("=")[::2] for arg in sys.argv[2:])
VERSION = MODE.split("=", 1)[1] if MODE.startswith("--version=") else "2025-06-18"

PAGES = [
    [
        {
            "name": "echo",
            "description": "Echoes its text.",
            "inputSchema": {
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
            },
        },
        {"name": "fail", "description": "Always fails.", "inputSchema": {"type": "object"}},
        {"name": "image", "description": "Returns a picture.", "inputSchema": {"type": "object"}},
        {"name": "bad.name", "description": "Not a valid tool name for providers."},
    ],
    [
        {"name": "structured", "description": "Structured output only."},
        {"name": "slow", "description": "Never answers.", "inputSchema": {"type": "object"}},
        {"name": "cancelled", "description": "Cancelled request IDs."},
        {"name": "die", "description": "Exits during the call."},
        {"name": "env", "description": "Environment variable names."},
        {"name": "pong", "description": "Whether the ping was answered."},
        {"name": "where", "description": "Its directory and PATH."},
        {"name": "deaf", "description": "Stops reading its input."},
        {"name": "malformed", "description": "Neither result nor error."},
        {"name": "empty", "description": "A result without content."},
        {"name": "batch", "description": "Answers inside a batch."},
        {"name": "secret", "description": "Repeats FAKE_SECRET in an error."},
        {"name": "expire", "description": "Ends every HTTP session."},
        {"name": "reflect", "description": "Repeats the Authorization header in an error."},
    ],
]
if MODE == "--many":
    PAGES[1].extend({"name": "t%d" % n, "description": "Tool %d." % n} for n in range(80))
if MODE == "--heavy":
    PAGES[1].extend(
        {"name": "h%d" % n, "description": "Heavy.", "inputSchema": {"type": "object", "description": "x" * 50000}}
        for n in range(4)
    )
if MODE == "--big":
    PAGES[1].append({"name": "wordy", "description": "w" * 3000})
    PAGES[1].append({
        "name": "huge",
        "description": "A huge schema.",
        "inputSchema": {"type": "object", "description": "x" * 70000},
    })

cancelled = []
pong = []


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def text(value, is_error=False):
    result = {"content": [{"type": "text", "text": value}]}
    if is_error:
        result["isError"] = True
    return result


def call(name, arguments):
    if name == "echo":
        return text("echo: " + arguments.get("text", ""))
    if name == "fail":
        return text("it broke", is_error=True)
    if name == "image":
        return {
            "content": [
                {"type": "text", "text": "a chart"},
                {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"},
            ]
        }
    if name == "structured":
        return {"content": [], "structuredContent": {"temperature": 21}}
    if name == "slow":
        return None
    if name == "cancelled":
        return text(",".join(str(i) for i in cancelled))
    if name == "die":
        sys.stderr.write("fatal: the database is gone\n")
        sys.stderr.flush()
        os._exit(3)
    if name == "env":
        return text(",".join(sorted(os.environ)))
    if name == "pong":
        return text(json.dumps(pong))
    if name == "where":
        return text(os.getcwd() + "\n" + os.environ.get("PATH", ""))
    if name == "deaf":
        time.sleep(3600)
    if name == "expire":
        with state:
            sessions.clear()
        return text("expired")
    raise KeyError(name)


def log_method(method):
    path = os.environ.get("FAKE_LOG")
    if path and method:
        with open(path, "a") as file:
            file.write(method + "\n")


def main():
    sys.stderr.write("fake server starting\n")
    sys.stderr.flush()
    if MODE == "--crash":
        sys.stderr.write("error: missing credentials\n")
        if "FAKE_SECRET" in os.environ:
            sys.stderr.write("token=" + os.environ["FAKE_SECRET"] + "\n")
        sys.exit(1)
    for line in sys.stdin:
        message = json.loads(line)
        method = message.get("method")
        log_method(method)
        if method is None:
            if message.get("id") == "server-ping":
                pong.append(message.get("result"))
            continue
        if method == "notifications/initialized":
            send({"jsonrpc": "2.0", "id": "server-ping", "method": "ping"})
            send({"jsonrpc": "2.0", "method": "notifications/message", "params": {"level": "info", "data": "ready"}})
            continue
        if method == "notifications/cancelled":
            cancelled.append(message["params"]["requestId"])
            continue
        if "id" not in message:
            continue
        reply = {"jsonrpc": "2.0", "id": message["id"]}
        if method == "initialize":
            if MODE == "--hang":
                continue
            reply["result"] = {
                "protocolVersion": VERSION,
                "capabilities": {} if MODE == "--no-tools" else {"tools": {}},
                "serverInfo": {"name": "fake", "version": "1"},
            }
        elif method == "tools/list":
            page = int((message.get("params") or {}).get("cursor") or 0)
            reply["result"] = {"tools": PAGES[page]}
            if page + 1 < len(PAGES):
                reply["result"]["nextCursor"] = str(page + 1)
        elif method == "tools/call":
            params = message["params"]
            if params["name"] == "malformed":
                send(reply)
                continue
            if params["name"] == "empty":
                reply["result"] = {}
                send(reply)
                continue
            if params["name"] == "batch":
                reply["result"] = text("batched")
                sys.stdout.write(json.dumps([reply]) + "\n")
                sys.stdout.flush()
                continue
            if params["name"] == "secret":
                reply["error"] = {"code": -32000, "message": "bad token " + os.environ.get("FAKE_SECRET", "")}
                send(reply)
                continue
            try:
                result = call(params["name"], params.get("arguments") or {})
            except KeyError:
                reply["error"] = {"code": -32602, "message": "Unknown tool: " + params["name"]}
                send(reply)
                continue
            if result is None:
                continue
            reply["result"] = result
        else:
            reply["error"] = {"code": -32601, "message": "Method not found"}
        send(reply)
    if MODE.startswith("--on-eof="):
        with open(MODE.split("=", 1)[1], "w") as file:
            file.write("closed\n")


# The HTTP mode. Handlers run on their own threads and share this state.
state = threading.Condition()
sessions = set()
HTTP_VERSION = "2025-03-26"


def log(entry):
    path = OPTIONS.get("log")
    if path:
        with state, open(path, "a") as file:
            file.write(json.dumps(entry) + "\n")


def wait_for(predicate, seconds=30):
    deadline = time.time() + seconds
    with state:
        while not predicate() and time.time() < deadline:
            state.wait(0.05)


def serve_http():
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def reply(self, status, body=None, headers=()):
            data = b"" if body is None else json.dumps(body).encode()
            self.send_response(status)
            for name, value in headers:
                self.send_header(name, value)
            if body is not None:
                self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def authorized(self):
            token = OPTIONS.get("token")
            return not token or self.headers.get("Authorization") == "Bearer " + token

        def do_GET(self):
            self.reply(405)

        def do_DELETE(self):
            session = self.headers.get("Mcp-Session-Id")
            log({"http": "DELETE", "session": session})
            if not self.authorized():
                return self.reply(401)
            with state:
                sessions.discard(session)
            self.reply(200)

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            message = json.loads(body)
            method = message.get("method")
            session = self.headers.get("Mcp-Session-Id")
            version = self.headers.get("MCP-Protocol-Version")
            log({"http": "POST", "rpc": method or "response", "session": session, "version": version})
            accept = self.headers.get("Accept", "")
            if "application/json" not in accept or "text/event-stream" not in accept:
                return self.reply(406)
            if self.headers.get("Content-Type") != "application/json":
                return self.reply(415)
            if not self.authorized():
                return self.reply(401)
            if method == "initialize":
                with state:
                    new = "session-%d" % (len(sessions) + 1)
                    sessions.add(new)
                result = {
                    "protocolVersion": HTTP_VERSION,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake", "version": "1"},
                }
                return self.answer(message["id"], result, [("Mcp-Session-Id", new)])
            if session is None:
                return self.reply(400, {"jsonrpc": "2.0", "error": {"code": -32600, "message": "missing session"}})
            with state:
                known = session in sessions
            if not known:
                return self.reply(404)
            if version != HTTP_VERSION:
                return self.reply(400, {"jsonrpc": "2.0", "error": {"code": -32600, "message": "bad protocol version"}})
            if method is None:
                if message.get("id") == "server-ping":
                    with state:
                        pong.append(message.get("result"))
                        state.notify_all()
                return self.reply(202)
            if "id" not in message:
                if method == "notifications/cancelled":
                    with state:
                        cancelled.append(message["params"]["requestId"])
                        state.notify_all()
                return self.reply(202)
            if method == "tools/list":
                page = int((message.get("params") or {}).get("cursor") or 0)
                result = {"tools": PAGES[page]}
                if page + 1 < len(PAGES):
                    result["nextCursor"] = str(page + 1)
                return self.answer(message["id"], result)
            if method == "tools/call":
                params = message["params"]
                if params["name"] == "reflect":
                    error = {"code": -32000, "message": "rejected " + self.headers.get("Authorization", "")}
                    return self.reply(400, {"jsonrpc": "2.0", "id": message["id"], "error": error})
                return self.answer(message["id"], None, call_name=params["name"], arguments=params.get("arguments") or {})
            self.reply(200, {"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "Method not found"}})

        def answer(self, id, result, headers=(), call_name=None, arguments=None):
            sse = "sse" in OPTIONS
            if sse:
                self.send_response(200)
                for name, value in headers:
                    self.send_header(name, value)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Connection", "close")
                self.end_headers()
                self.close_connection = True
                if "cr" in OPTIONS:
                    self.wfile.write(b"\xef\xbb\xbf")
                self.event({"jsonrpc": "2.0", "method": "notifications/message", "params": {"level": "info", "data": "working"}})
                self.wfile.write(b": keep-alive\r\r" if "cr" in OPTIONS else b": keep-alive\n\n")
                self.wfile.flush()
            reply = {"jsonrpc": "2.0", "id": id}
            if call_name is not None:
                if call_name == "slow":
                    wait_for(lambda: id in cancelled)
                    return
                if call_name == "pong" and sse:
                    self.event({"jsonrpc": "2.0", "id": "server-ping", "method": "ping"})
                    wait_for(lambda: pong, 5)
                try:
                    result = call(call_name, arguments)
                except KeyError:
                    reply["error"] = {"code": -32602, "message": "Unknown tool: " + call_name}
            if "error" not in reply:
                reply["result"] = result
            if sse:
                self.event(reply, multiline=True)
            else:
                self.reply(200, reply, headers)

        def event(self, message, multiline=False):
            end = "\r" if "cr" in OPTIONS else "\n"
            text = json.dumps(message, indent=1 if multiline else None)
            data = "".join("data: " + line + end for line in text.split("\n"))
            self.wfile.write(("event: message" + end + "id: %d" % time.time_ns() + end + data + end).encode())
            self.wfile.flush()

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    sys.stdout.write("PORT %d\n" % server.server_address[1])
    sys.stdout.flush()
    server.serve_forever()


if MODE == "--http":
    serve_http()
else:
    main()
