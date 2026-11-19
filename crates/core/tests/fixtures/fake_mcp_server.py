"""A small MCP server over stdio for Socium's client tests.

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
(its answer inside a batch) and `secret` (an error that repeats
`FAKE_SECRET`).
"""

import json
import os
import sys
import time

MODE = sys.argv[1] if len(sys.argv) > 1 else ""
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
    raise KeyError(name)


def log(method):
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
        log(method)
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


main()
