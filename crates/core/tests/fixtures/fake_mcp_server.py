"""A small MCP server over stdio for Socium's client tests.

`--crash` exits during startup and `--hang` never answers `initialize`.
Otherwise it lists its tools over two pages and answers `tools/call`:
`echo`, `fail` (isError), `image`, `structured`, `slow` (never answers,
so the caller must cancel), `cancelled` (the request IDs the client
cancelled), `die` (exits mid-call), `env` (the variable names it sees) and
`pong` (whether the client answered its ping).
"""

import json
import os
import sys

MODE = sys.argv[1] if len(sys.argv) > 1 else ""

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
    ],
]

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
    raise KeyError(name)


def main():
    sys.stderr.write("fake server starting\n")
    sys.stderr.flush()
    if MODE == "--crash":
        sys.stderr.write("error: missing credentials\n")
        sys.exit(1)
    for line in sys.stdin:
        message = json.loads(line)
        method = message.get("method")
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
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "fake", "version": "1"},
            }
        elif method == "tools/list":
            page = int((message.get("params") or {}).get("cursor") or 0)
            reply["result"] = {"tools": PAGES[page]}
            if page + 1 < len(PAGES):
                reply["result"]["nextCursor"] = str(page + 1)
        elif method == "tools/call":
            params = message["params"]
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


main()
