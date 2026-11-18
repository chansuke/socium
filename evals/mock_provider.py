#!/usr/bin/env python3
"""A loopback OpenAI-compatible model that plays a task's reference solution.

`run.py --mock PLAY` starts one per run, so the runner, the validators, and
the scope check can be tested through the real binary without a paid model.
PLAY is one of:

    solution   write the files in the task's solution/ on the last turn,
               then reply with reply.txt
    nothing    reply "Done." without touching the repository
    stray      the solution, plus a file the task did not ask for

Earlier turns of a multi-turn task are answered with a short text. Every
response reports usage, so token and cost accounting run as in a real run.

usage: mock_provider.py --task DIR --repo DIR --turns N --play PLAY --port-file FILE
"""
import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

EDITOR = "str_replace_based_edit_tool"
STRAY = "notes-from-the-agent.txt"
COMPLETION_TOKENS = 40


def solution_calls(task, repo, play):
    """One editor call per file in solution/, against the file as it is now."""
    calls = []
    if play in ("solution", "stray"):
        solution = task / "solution"
        for source in sorted(p for p in solution.rglob("*") if p.is_file()):
            path = source.relative_to(solution).as_posix()
            wanted = source.read_bytes().decode("utf-8")
            target = repo / path
            if target.exists():
                current = target.read_bytes().decode("utf-8")
                if current == wanted:
                    continue
                args = {"command": "str_replace", "path": path, "old_str": current, "new_str": wanted}
            else:
                args = {"command": "create", "path": path, "file_text": wanted}
            calls.append(args)
    if play == "stray":
        calls.append({"command": "create", "path": STRAY, "file_text": "unrequested\n"})
    return calls


def reply_text(task, play):
    reply = task / "reply.txt"
    return reply.read_text() if reply.exists() and play != "nothing" else "Done."


def respond(body, task, repo, turns, play):
    """The chunks for one request: text, or the solution's editor calls."""
    messages = body.get("messages", [])
    turn = sum(1 for m in messages if m.get("role") == "user")
    last = messages[-1].get("role") if messages else "user"
    if last == "user" and turn < turns:
        return [{"content": "Noted."}], "stop"
    if last == "user":
        calls = solution_calls(task, repo, play)
        if calls:
            return [
                {
                    "tool_calls": [
                        {
                            "index": i,
                            "id": f"call_{turn}_{i}",
                            "type": "function",
                            "function": {"name": EDITOR, "arguments": json.dumps(args)},
                        }
                        for i, args in enumerate(calls)
                    ]
                }
            ], "tool_calls"
    return [{"content": reply_text(task, play)}], "stop"


def make_handler(task, repo, turns, play):
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            raw = self.rfile.read(length)
            body = json.loads(raw)
            deltas, finish = respond(body, task, repo, turns, play)
            chunks = [
                {"id": "mock", "choices": [{"index": 0, "delta": delta, "finish_reason": None}]}
                for delta in deltas
            ]
            chunks.append({"id": "mock", "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]})
            chunks.append(
                {
                    "id": "mock",
                    "choices": [],
                    "usage": {"prompt_tokens": len(raw) // 4, "completion_tokens": COMPLETION_TOKENS},
                }
            )
            payload = "".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n"
            data = payload.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args):
            pass

    return Handler


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--task", type=Path, required=True)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--turns", type=int, required=True)
    parser.add_argument("--play", choices=["solution", "nothing", "stray"], required=True)
    parser.add_argument("--port-file", type=Path, required=True)
    args = parser.parse_args()
    server = ThreadingHTTPServer(
        ("127.0.0.1", 0), make_handler(args.task, args.repo, args.turns, args.play)
    )
    args.port_file.write_text(str(server.server_address[1]))
    server.serve_forever()


if __name__ == "__main__":
    sys.exit(main())
