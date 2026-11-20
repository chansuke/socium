#!/usr/bin/env python3
"""Run the packaged socium from an empty environment and check that the git
and ripgrep tools it ships with work.

A loopback OpenAI-compatible model asks for search_text, git_status and
git_log once, then answers. The binary runs with only HOME, its config and
a PATH entry that does not exist, so it gets git and rg from the package's
wrapper. The tools also look in /usr/bin and /bin whatever PATH says, so
BWRAP runs socium in a mount namespace where the host's git and rg there
are replaced by /dev/null; the same bwrap must be allowed to create user
namespaces, as it is for Socium's own sandbox.

usage: package_smoke.py SOCIUM BWRAP
"""
import json
import os
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

CALLS = [
    ("search_text", {"pattern": "smoke-needle"}),
    ("git_status", {}),
    ("git_log", {"path": None, "max_count": None}),
]


def reply(body):
    """Tool calls for the user's message, then a text answer."""
    last = body["messages"][-1]["role"]
    if last == "user":
        delta = {"tool_calls": [
            {"index": i, "id": f"call_{i}", "type": "function",
             "function": {"name": name, "arguments": json.dumps(args)}}
            for i, (name, args) in enumerate(CALLS)
        ]}
        return delta, "tool_calls"
    return {"content": "done"}, "stop"


class Model(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        delta, finish = reply(body)
        chunks = [
            {"id": "smoke", "choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
            {"id": "smoke", "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]},
            {"id": "smoke", "choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 10}},
        ]
        data = ("".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


def git(repo, *args):
    # The host's git only builds the fixture; the run under test never sees it.
    subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True,
                   env={"GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1",
                        "PATH": "/usr/bin:/bin", "HOME": str(repo)})


def without_host_programs(bwrap, argv):
    """`argv` under `bwrap`, with every host git and rg in /usr/bin and /bin
    hidden behind /dev/null."""
    hidden = sorted({os.path.realpath(f"{d}/{p}") for d in ("/usr/bin", "/bin") for p in ("git", "rg")
                     if os.path.isfile(f"{d}/{p}")})
    hide = [arg for path in hidden for arg in ("--ro-bind", "/dev/null", path)]
    return [bwrap, "--unshare-user", "--dev-bind", "/", "/", *hide, "--", *argv], hidden


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__.rsplit("usage: ", 1)[1])
    socium, bwrap = Path(sys.argv[1]).resolve(), sys.argv[2]
    server = ThreadingHTTPServer(("127.0.0.1", 0), Model)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with tempfile.TemporaryDirectory(prefix="socium-package-smoke-") as tmp:
        tmp = Path(tmp)
        repo, home = tmp / "repo", tmp / "home"
        repo.mkdir()
        home.mkdir()
        (repo / "notes.txt").write_text("smoke-needle\n")
        git(repo, "init", "-q")
        git(repo, "add", "notes.txt")
        git(repo, "-c", "user.name=smoke", "-c", "user.email=smoke@example.invalid",
            "commit", "-q", "-m", "first")
        (repo / "notes.txt").write_text("smoke-needle\nchanged\n")
        models = tmp / "models.toml"
        models.write_text(
            'default = "smoke"\n'
            "[providers.smoke]\n"
            'wire = "openai-chat"\n'
            f'base_url = "http://127.0.0.1:{server.server_address[1]}/v1"\n'
            "[[models]]\n"
            'alias = "smoke"\nprovider = "smoke"\nid = "smoke-model"\n'
            "context_window = 200000\nmax_output_tokens = 8000\n"
        )
        env = {
            "PATH": str(tmp / "no-such-dir"),
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(home / ".config"),
            "SOCIUM_MODELS_FILE": str(models),
        }
        argv, hidden = without_host_programs(
            bwrap, [str(socium), "-p", "smoke", "--output-format", "stream-json"])
        done = subprocess.run(
            argv,
            cwd=repo, env=env, capture_output=True, text=True, timeout=120,
        )
    server.shutdown()
    records = [json.loads(line) for line in done.stdout.splitlines() if line.startswith("{")]
    events = [r["event"] for r in records if r.get("type") == "activity"]
    finished = [e for e in events if e.get("type") == "tool_finished"]
    problems = []
    if done.returncode != 0:
        problems.append(f"exit status {done.returncode}")
    for name, _ in CALLS:
        runs = [e for e in finished if e.get("name") == name]
        if not runs:
            problems.append(f"{name} did not run")
        elif any(e.get("is_error") for e in runs):
            problems.append(f"{name} failed: {runs[0].get('summary')}")
    if "not installed" in done.stderr:
        problems.append("a program the tools need was reported missing")
    if problems:
        print("\n".join(problems))
        print(f"--- stderr\n{done.stderr}--- stdout\n{done.stdout}")
        return 1
    print(f"ok: {', '.join(name for name, _ in CALLS)} ran from an empty PATH, "
          f"with {', '.join(hidden) or 'no host git or rg'} hidden")
    return 0


if __name__ == "__main__":
    sys.exit(main())
