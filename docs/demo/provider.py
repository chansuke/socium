#!/usr/bin/env python3
"""A loopback OpenAI-compatible model that plays the README demo.

The demo workspace has a slugify function that forgets to lowercase its input.
Each request is answered by the next step of a fixed script, chosen by how many
tool results the conversation already holds: run the tests, read the file,
fix it, run the tests again, and reply. Text and reasoning are streamed in
small pieces with pauses so the recording shows them arrive.

usage: provider.py --port-file FILE
"""
import argparse
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

TESTS = ["python3", "-m", "unittest", "-v"]
BUGGY = '    words = re.findall(r"[a-z0-9]+", title)\n'
FIXED = '    words = re.findall(r"[a-z0-9]+", title.lower())\n'

STEPS = [
    ("Start by running the test suite to see what fails.",
     "run_command", {"command": TESTS}),
    ("The test expects lowercase words but slugify drops capital letters. "
     "Read slug.py.",
     "read_file", {"path": "slug.py"}),
    ("The pattern only matches lowercase letters, so lowercase the title "
     "before matching.",
     "str_replace_based_edit_tool",
     {"command": "str_replace", "path": "slug.py", "old_str": BUGGY, "new_str": FIXED}),
    ("Run the tests again to confirm the fix.",
     "run_command", {"command": TESTS}),
]
REPLY = (
    "Fixed. `slugify` matched only `[a-z0-9]`, so capital letters were "
    "dropped and \"Hello World\" became `ello-orld`. It now lowercases the "
    "title first (slug.py:6), and both tests pass."
)


def pieces(text, size=12):
    return [text[i:i + size] for i in range(0, len(text), size)]


def respond(body):
    """The deltas for one request and the finish reason."""
    done = sum(1 for m in body.get("messages", []) if m.get("role") == "tool")
    if done >= len(STEPS):
        return [{"content": p} for p in pieces(REPLY)], "stop"
    reasoning, name, args = STEPS[done]
    deltas = [{"reasoning_content": p} for p in pieces(reasoning)]
    deltas.append({
        "tool_calls": [{
            "index": 0,
            "id": f"call_{done}",
            "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)},
        }]
    })
    return deltas, "tool_calls"


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        deltas, finish = respond(json.loads(raw))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

        def send(chunk, pause=0.0):
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
            self.wfile.flush()
            time.sleep(pause)

        time.sleep(0.6)
        for delta in deltas:
            send({"id": "demo", "choices": [{"index": 0, "delta": delta, "finish_reason": None}]}, 0.04)
        send({"id": "demo", "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]})
        send({"id": "demo", "choices": [],
              "usage": {"prompt_tokens": len(raw) // 4, "completion_tokens": 60}})
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def log_message(self, *args):
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port-file", type=Path, required=True)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    args.port_file.write_text(str(server.server_address[1]))
    server.serve_forever()


if __name__ == "__main__":
    sys.exit(main())
