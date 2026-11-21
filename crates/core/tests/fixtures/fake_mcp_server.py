"""A small MCP server over stdio or Streamable HTTP for Socium's client tests.

`--crash` exits during startup, writing `FAKE_SECRET` after `FAKE_PAD`
x's to standard error when set, and `--hang` never answers `initialize`.
`--version=V` answers `initialize` with revision V, `--no-tools` offers no
tools capability, `--big` adds a tool with a long description and one with
a huge schema, `--many` adds 80 small tools, `--heavy` adds four with
schemas of about 50 KB, and `--on-eof=PATH` writes PATH when its input closes. With
`FAKE_LOG` set it appends each method it receives to that file.

`--probe` adds tools that report what the server can reach, for the
sandbox tests: `read` (a file's text), `write` (writes a file), `connect`
(a TCP connection to `host`:`port`), `getenv` (one variable, or "unset"),
`listdir` (a directory's names) and `detach` (starts a grandchild in a new
session that sleeps with `tag` in its command line). After `--probe`,
`--linger` keeps running once its input closes, `--linger=term` ignores
SIGTERM as well, and `--tag=T` only marks its command line.
Otherwise it lists its tools over two pages and answers `tools/call`:
`echo`, `fail` (isError), `image`, `structured`, `slow` (never answers,
so the caller must cancel), `cancelled` (the request IDs the client
cancelled), `die` (exits mid-call), `env` (the variable names it sees),
`pong` (whether the client answered its ping), `where` (its directory and
PATH), `deaf` (stops reading its input), `malformed` (an answer with
neither result nor error), `empty` (a result without content), `batch`
(its answer inside a batch), `secret` (an error that repeats
`FAKE_SECRET`) and `expire` (over HTTP, ends every session and revokes every access token).

`--http` serves the MCP endpoint `/mcp` on 127.0.0.1 and prints
`PORT <n>` once it listens. With `--sse` every request is answered with an
event stream that carries a notification first (and a ping for `pong`);
otherwise with one JSON body. `--cr` starts each stream with a BOM and ends
its lines with CR alone. Over HTTP, `reflect` repeats the Authorization
header after `pad` x's: in an HTTP 400 error (`how` "http"), a JSON-RPC
error (`how` "rpc"), or a tool error result (`how` "result").
`--reflect-initialize` answers every initialize as `reflect` does.
`--token=T` requires `Authorization: Bearer T`. `--log=PATH` appends one
JSON line per HTTP request: the HTTP method, the JSON-RPC method, and the
session and protocol version headers.
`--content-type=T` gives JSON answers that content type, and
`--notify-status=N` accepts notifications with N instead of 202.
Over HTTP, `slow` with `then` "404" answers 404 once a later session has
started.

`--oauth` makes the same process the MCP server's OAuth authorization
server too (issuer `/auth`): the MCP endpoint then takes only access tokens
it issued, and answers 401 with `WWW-Authenticate`. The client finds the
protected resource metadata from that header's `resource_metadata`
(`--discovery=header`, the default) or at the well-known URL
(`--discovery=well-known`), and the authorization server metadata at
RFC 8414's URL (`--as-metadata=oauth`, the default) or OpenID Connect's
(`--as-metadata=oidc`). `--no-registration` leaves out dynamic client
registration; then only the client `pre-client` with the secret
`pre-secret` exists. `--expires=N` sets the access token lifetime in
seconds, `--refresh=fail` refuses every refresh, and `--scopes=a,b` lists
`scopes_supported`. `--no-s256` leaves `code_challenge_methods_supported`
out, `--no-resource` leaves `resource` out of the protected resource
metadata, `--token-delay=S` holds each token answer S seconds,
`--refresh-delay=S` holds each refresh answer S seconds more, and
`--registration-auth=M` registers clients with a secret and the
authentication method M (`omit` answers with neither). `/authorize` redirects at once, as if the user agreed;
`/token` checks the PKCE verifier, the redirect URI and the resource. Each
authorization request is logged with its path and parameters.
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
if MODE == "--probe":
    PAGES[1].extend(
        {"name": name, "description": "Probe: " + name + "."}
        for name in ["read", "write", "connect", "getenv", "listdir", "detach"]
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
    if MODE == "--probe" and name in PROBES:
        try:
            return text(PROBES[name](arguments))
        except Exception as error:
            return text("%s: %s" % (type(error).__name__, error), is_error=True)
    if name == "expire":
        with state:
            sessions.clear()
            access_tokens.clear()
        return text("expired")
    raise KeyError(name)


def probe_read(arguments):
    with open(arguments["path"]) as file:
        return file.read()


def probe_write(arguments):
    with open(arguments["path"], "w") as file:
        file.write("written by the server\n")
    return "wrote"


def probe_connect(arguments):
    import socket

    with socket.create_connection((arguments["host"], int(arguments["port"])), timeout=2):
        return "connected"


def probe_detach(arguments):
    import subprocess

    child = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(3600)", arguments["tag"]],
        start_new_session=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return str(child.pid)


PROBES = {
    "read": probe_read,
    "write": probe_write,
    "connect": probe_connect,
    "getenv": lambda arguments: os.environ.get(arguments["name"], "unset"),
    "listdir": lambda arguments: ",".join(sorted(os.listdir(arguments["path"]))),
    "detach": probe_detach,
}


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
            pad = "x" * int(os.environ.get("FAKE_PAD", "0"))
            sys.stderr.write("token=" + pad + os.environ["FAKE_SECRET"] + "\n")
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
    if "linger" in OPTIONS:
        if OPTIONS["linger"] == "term":
            import signal

            signal.signal(signal.SIGTERM, signal.SIG_IGN)
        while True:
            time.sleep(3600)


# The HTTP mode. Handlers run on their own threads and share this state.
state = threading.Condition()
sessions = set()
# How many sessions were ever started, which numbers them.
started = [0]
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


# The authorization server's state.
clients = {}
codes = {}
access_tokens = {}
refresh_tokens = {}
counter = [0]


def next_id(prefix):
    with state:
        counter[0] += 1
        return "%s-%d" % (prefix, counter[0])


def serve_http():
    import base64
    import hashlib
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from urllib.parse import parse_qs, urlencode, urlsplit

    if "no-registration" in OPTIONS:
        clients["pre-client"] = {"secret": "pre-secret", "redirect_uris": None}

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
            given = self.headers.get("Authorization", "")
            if "oauth" in OPTIONS:
                token = given[len("Bearer "):] if given.startswith("Bearer ") else None
                with state:
                    expires = access_tokens.get(token)
                return expires is not None and expires > time.time()
            token = OPTIONS.get("token")
            return not token or given == "Bearer " + token

        def unauthorized(self):
            if "oauth" not in OPTIONS:
                return self.reply(401)
            if OPTIONS.get("discovery", "header") == "header":
                challenge = 'Bearer error="invalid_token", resource_metadata="%s/meta/resource"' % self.origin()
            else:
                challenge = 'Bearer realm="mcp"'
            self.reply(401, None, [("WWW-Authenticate", challenge)])

        def origin(self):
            return "http://127.0.0.1:%d" % self.server.server_address[1]

        def do_GET(self):
            url = urlsplit(self.path)
            params = {key: values[0] for key, values in parse_qs(url.query).items()}
            if "oauth" not in OPTIONS:
                return self.reply(405)
            log({"http": "GET", "path": url.path, "params": params})
            origin = self.origin()
            discovery = OPTIONS.get("discovery", "header")
            prm_path = "/meta/resource" if discovery == "header" else "/.well-known/oauth-protected-resource/mcp"
            if url.path == prm_path:
                metadata = {"resource": origin + "/mcp", "authorization_servers": [origin + "/auth"]}
                if "no-resource" in OPTIONS:
                    del metadata["resource"]
                if OPTIONS.get("scopes"):
                    metadata["scopes_supported"] = OPTIONS["scopes"].split(",")
                return self.reply(200, metadata)
            as_path = {
                "oauth": "/.well-known/oauth-authorization-server/auth",
                "oidc": "/auth/.well-known/openid-configuration",
            }[OPTIONS.get("as-metadata", "oauth")]
            if url.path == as_path:
                metadata = {
                    "issuer": origin + "/auth",
                    "authorization_endpoint": origin + "/auth/authorize",
                    "token_endpoint": origin + "/auth/token",
                    "response_types_supported": ["code"],
                    "code_challenge_methods_supported": ["S256"],
                }
                if "no-s256" in OPTIONS:
                    del metadata["code_challenge_methods_supported"]
                if "no-registration" not in OPTIONS:
                    metadata["registration_endpoint"] = origin + "/auth/register"
                return self.reply(200, metadata)
            if url.path == "/auth/authorize":
                return self.authorize(params)
            self.reply(404)

        def authorize(self, params):
            with state:
                client = clients.get(params.get("client_id"))
            redirect = params.get("redirect_uri")
            problems = []
            if client is None:
                problems.append("unknown client")
            elif client["redirect_uris"] is not None and redirect not in client["redirect_uris"]:
                problems.append("unregistered redirect_uri")
            if params.get("response_type") != "code":
                problems.append("response_type")
            if params.get("code_challenge_method") != "S256" or not params.get("code_challenge"):
                problems.append("PKCE")
            if params.get("resource") != self.origin() + "/mcp":
                problems.append("resource")
            if problems or not redirect:
                return self.reply(400, {"error": "invalid_request", "error_description": ", ".join(problems)})
            code = next_id("code")
            with state:
                codes[code] = {
                    "client_id": params["client_id"],
                    "redirect_uri": redirect,
                    "challenge": params["code_challenge"],
                    "resource": params["resource"],
                }
            query = {"code": code}
            if "state" in params:
                query["state"] = params["state"]
            self.reply(302, None, [("Location", redirect + "?" + urlencode(query))])

        def authenticate(self, form):
            """The client of a token request, or None."""
            client_id, secret = form.get("client_id"), form.get("client_secret")
            basic = self.headers.get("Authorization", "")
            if basic.startswith("Basic "):
                client_id, _, secret = base64.b64decode(basic[6:]).decode().partition(":")
            with state:
                client = clients.get(client_id)
            if client is None or client["secret"] != secret:
                return None
            return client_id

        def token(self, form):
            time.sleep(float(OPTIONS.get("token-delay", "0")))
            client_id = self.authenticate(form)
            if client_id is None:
                return self.reply(401, {"error": "invalid_client"})
            if form.get("resource") != self.origin() + "/mcp":
                return self.reply(400, {"error": "invalid_target"})
            grant = form.get("grant_type")
            if grant == "authorization_code":
                with state:
                    code = codes.pop(form.get("code"), None)
                if code is None or code["client_id"] != client_id or code["redirect_uri"] != form.get("redirect_uri"):
                    return self.reply(400, {"error": "invalid_grant"})
                verifier = form.get("code_verifier", "")
                challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
                if challenge != code["challenge"]:
                    return self.reply(400, {"error": "invalid_grant", "error_description": "PKCE verification failed"})
            elif grant == "refresh_token":
                time.sleep(float(OPTIONS.get("refresh-delay", "0")))
                with state:
                    owner = refresh_tokens.pop(form.get("refresh_token"), None)
                if OPTIONS.get("refresh") == "fail" or owner != client_id:
                    return self.reply(400, {"error": "invalid_grant"})
            else:
                return self.reply(400, {"error": "unsupported_grant_type"})
            lifetime = int(OPTIONS.get("expires", "3600"))
            access, refresh = next_id("access"), next_id("refresh")
            with state:
                access_tokens[access] = time.time() + lifetime
                refresh_tokens[refresh] = client_id
            self.reply(200, {"access_token": access, "token_type": "Bearer", "expires_in": lifetime, "refresh_token": refresh})

        def register(self, body):
            client_id = next_id("client")
            method = OPTIONS.get("registration-auth")
            secret = next_id("secret") if method and method != "omit" else None
            with state:
                clients[client_id] = {"secret": secret, "redirect_uris": body.get("redirect_uris")}
            answer = {"client_id": client_id, "redirect_uris": body.get("redirect_uris")}
            if method != "omit":
                answer["token_endpoint_auth_method"] = method or "none"
            if secret:
                answer["client_secret"] = secret
            self.reply(201, answer)

        def do_DELETE(self):
            session = self.headers.get("Mcp-Session-Id")
            log({"http": "DELETE", "session": session})
            if not self.authorized():
                return self.unauthorized()
            with state:
                sessions.discard(session)
            self.reply(200)

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            if self.path.startswith("/auth/"):
                if self.path == "/auth/token":
                    form = {key: values[0] for key, values in parse_qs(body.decode()).items()}
                    log({"http": "POST", "path": self.path, "params": form, "basic": self.headers.get("Authorization")})
                    return self.token(form)
                if self.path == "/auth/register" and "no-registration" not in OPTIONS:
                    request = json.loads(body)
                    log({"http": "POST", "path": self.path, "params": request})
                    return self.register(request)
                return self.reply(404)
            message = json.loads(body)
            method = message.get("method")
            session = self.headers.get("Mcp-Session-Id")
            version = self.headers.get("MCP-Protocol-Version")
            log({"http": "POST", "rpc": method or "response", "session": session, "version": version, "auth": self.headers.get("Authorization")})
            accept = self.headers.get("Accept", "")
            if "application/json" not in accept or "text/event-stream" not in accept:
                return self.reply(406)
            if self.headers.get("Content-Type") != "application/json":
                return self.reply(415)
            if method == "initialize" and "reflect-initialize" in OPTIONS:
                return self.reflect(message["id"], {})
            if not self.authorized():
                return self.unauthorized()
            if method == "initialize":
                with state:
                    started[0] += 1
                    new = "session-%d" % started[0]
                    sessions.add(new)
                    state.notify_all()
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
                return self.reply(int(OPTIONS.get("notify-status", "202")))
            if "id" not in message:
                if method == "notifications/cancelled":
                    with state:
                        cancelled.append(message["params"]["requestId"])
                        state.notify_all()
                return self.reply(int(OPTIONS.get("notify-status", "202")))
            if method == "tools/list":
                page = int((message.get("params") or {}).get("cursor") or 0)
                result = {"tools": PAGES[page]}
                if page + 1 < len(PAGES):
                    result["nextCursor"] = str(page + 1)
                return self.answer(message["id"], result)
            if method == "tools/call":
                params = message["params"]
                arguments = params.get("arguments") or {}
                if params["name"] == "reflect":
                    return self.reflect(message["id"], arguments)
                if params["name"] == "slow" and arguments.get("then") == "404":
                    with state:
                        before = started[0]
                    wait_for(lambda: started[0] > before, 10)
                    return self.reply(404)
                return self.answer(message["id"], None, call_name=params["name"], arguments=params.get("arguments") or {})
            self.reply(200, {"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "Method not found"}})

        def reflect(self, id, arguments):
            said = "rejected " + "x" * int(arguments.get("pad", 0)) + self.headers.get("Authorization", "")
            how = arguments.get("how", "http")
            if how == "http":
                return self.reply(400, {"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": said}})
            if how == "rpc":
                return self.answer(id, None, error={"code": -32000, "message": said})
            return self.answer(id, text(said, is_error=True))

        def answer(self, id, result, headers=(), call_name=None, arguments=None, error=None):
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
            if error is not None:
                reply["error"] = error
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
            elif "content-type" in OPTIONS:
                data = json.dumps(reply).encode()
                self.send_response(200)
                for name, value in headers:
                    self.send_header(name, value)
                self.send_header("Content-Type", OPTIONS["content-type"])
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
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
