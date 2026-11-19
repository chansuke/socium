#!/usr/bin/env python3
"""Measure what Socium costs to run, apart from the model.

Each scenario in scenarios.toml is a fixed conversation played by a loopback
model (mock_model.py), so the same work happens on every run. For each
binary and scenario the runner records what the model received (requests and
bytes, which do not vary between runs) and what the run cost on this machine
(wall time, CPU time, peak memory, which do). Binaries are run in turn within
each repetition, so warmup and drift favour none of them.

A run counts only when Socium exits 0, the model served every call of the
scenario plus the final reply, the tool calls failed as often as the scenario
says, and the last request holds the scenario's `expect` texts, which only
the real tool output contains. Anything else is recorded as an error.

usage: run.py --binary LABEL=PATH [--binary LABEL=PATH ...] [--lean-ctx LABEL=PATH]
              [--repeat N] [--scenario NAME] --out FILE
"""
import argparse
import json
import os
import platform
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
CONFIG = ROOT / "evals" / "config.toml"
TIMEOUT_SECONDS = 120
STARTUP_SAMPLES = 5
NOISY = ("wall_ms", "cpu_ms", "max_rss_kib")
EXACT = ("requests", "request_bytes", "first_request_bytes", "tool_result_bytes")


def git(repo, *args):
    # No user config, a fixed identity and a fixed date keep the commit, and
    # so what git_log and git_status show, the same on every machine.
    env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
           "GIT_AUTHOR_NAME": "bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
           "GIT_COMMITTER_NAME": "bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid",
           "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z"}
    subprocess.run(["git", *args], cwd=repo, env=env, check=True, capture_output=True)


def prepare(scenario, workdir):
    repo = workdir / "repo"
    shutil.copytree(ROOT / scenario["repo"], repo)
    git(repo, "init", "-q", "-b", "main")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "fixture")
    (workdir / "calls.json").write_text(json.dumps(scenario["calls"]))
    home = workdir / "home"
    home.mkdir()
    return repo, home


def start_model(workdir):
    port_file = workdir / "port"
    server = subprocess.Popen([
        sys.executable, "-I", str(HERE / "mock_model.py"),
        "--calls", str(workdir / "calls.json"), "--log", str(workdir / "requests.jsonl"),
        "--port-file", str(port_file),
    ])
    for _ in range(200):
        if port_file.exists() and port_file.read_text():
            return server, int(port_file.read_text())
        time.sleep(0.025)
    server.kill()
    raise RuntimeError("the loopback model did not start")


def write_models(path, port):
    """A models.toml with one model at the loopback port. The key is fake;
    the lean-ctx proxy refuses requests without one."""
    path.write_text(
        "[providers.mock]\n"
        'wire = "openai-chat"\n'
        'api_key_env = "MOCK_API_KEY"\n'
        f'base_url = "http://127.0.0.1:{port}/v1"\n'
        "[[models]]\n"
        'alias = "mock"\nprovider = "mock"\nid = "mock-model"\n'
        "context_window = 200000\nmax_output_tokens = 8000\n"
    )
    return {"SOCIUM_MODELS_FILE": str(path), "SOCIUM_CONFIG_FILE": str(CONFIG),
            "MODEL": "mock", "MOCK_API_KEY": "sk-loopback-not-a-real-key"}


def start_lean_ctx(lean_ctx, workdir, upstream):
    """lean-ctx's compression proxy in front of the loopback model, with its
    own empty HOME. Cache mode leaves the system prompt and the user's words
    alone; the default token mode rewrites both."""
    home = workdir / "lean-ctx-home"
    home.mkdir()
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    proxy = subprocess.Popen(
        [lean_ctx, "proxy", "start", f"--port={port}"],
        env={"PATH": os.environ.get("PATH", ""), "HOME": str(home),
             "LEAN_CTX_OPENAI_UPSTREAM": f"http://127.0.0.1:{upstream}", "LEAN_CTX_PROXY_MODE": "cache"},
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    for _ in range(200):
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.1).close()
            return proxy, port
        except OSError:
            time.sleep(0.025)
    proxy.kill()
    raise RuntimeError("the lean-ctx proxy did not start")


def run_once(binary, scenario, lean_ctx=None):
    with tempfile.TemporaryDirectory(prefix="socium-bench-") as tmp:
        workdir = Path(tmp)
        repo, home = prepare(scenario, workdir)
        server, port = start_model(workdir)
        proxy = None
        if lean_ctx:
            proxy, port = start_lean_ctx(lean_ctx, workdir, port)
        env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(home / ".config"),
            **write_models(workdir / "models.toml", port),
        }
        output, errors = workdir / "output", workdir / "stderr"
        try:
            with output.open("wb") as sink, errors.open("wb") as error_sink:
                started = time.perf_counter()
                process = subprocess.Popen(
                    [binary, "-p", scenario["prompt"], "--output-format", "stream-json"], cwd=repo, env=env,
                    stdin=subprocess.DEVNULL, stdout=sink, stderr=error_sink,
                )
                timer = threading.Timer(TIMEOUT_SECONDS, process.kill)
                timer.start()
                # wait4 reports the child's own CPU time and peak memory,
                # including the sandboxed commands it waited for.
                _, status, usage = os.wait4(process.pid, 0)
                wall = time.perf_counter() - started
                timer.cancel()
                process.returncode = os.waitstatus_to_exitcode(status)
        finally:
            for helper in (server, proxy):
                if helper:
                    helper.kill()
                    helper.wait()
        log = workdir / "requests.jsonl"
        requests = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
        last = workdir / "last-request.json"
        last_request = last.read_text() if last.exists() else ""
        records = [json.loads(line) for line in output.read_text().splitlines() if line.startswith("{")]
        tail = errors.read_bytes().decode(errors="replace")[-500:]
    tool_errors = sum(
        1 for r in records
        if r.get("type") == "activity" and r["event"].get("type") == "tool_finished" and r["event"].get("is_error")
    )
    error = None
    if process.returncode != 0:
        error = f"exit {process.returncode}: {tail}"
    elif tool_errors != scenario.get("tool_errors", 0):
        error = f"{tool_errors} tool calls failed, expected {scenario.get('tool_errors', 0)}"
    elif missing := [text for text in scenario.get("expect", []) if text not in last_request]:
        error = f"the model never received {missing}"
    elif len(requests) != len(scenario["calls"]) + 1:
        error = f"model served {len(requests)} requests, expected {len(scenario['calls']) + 1}"
    return {
        "wall_ms": round(wall * 1000, 1),
        "cpu_ms": round((usage.ru_utime + usage.ru_stime) * 1000, 1),
        "max_rss_kib": usage.ru_maxrss,
        "requests": len(requests),
        "request_bytes": sum(r["bytes"] for r in requests),
        "first_request_bytes": requests[0]["bytes"] if requests else 0,
        "tool_result_bytes": requests[-1]["tool_result_bytes"] if requests else 0,
        "error": error,
    }


def startup_ms(binary):
    started = time.perf_counter()
    subprocess.run([binary, "--help"], check=True, capture_output=True, env={"PATH": os.environ.get("PATH", "")})
    return round((time.perf_counter() - started) * 1000, 2)


def executable_size(binary):
    """The size of the program itself; the Nix package's bin/socium is a
    wrapper script that execs bin/.socium-wrapped."""
    path = Path(binary)
    wrapped = path.with_name(f".{path.name}-wrapped")
    return (wrapped if wrapped.exists() else path).stat().st_size


def closure_bytes(binary):
    """The size of the binary's Nix store path with everything it needs, or
    None outside the store."""
    store = Path(binary).resolve()
    parts = store.parts
    if len(parts) < 4 or parts[1] != "nix":
        return None
    path = "/".join(parts[:4])
    try:
        out = subprocess.run(["nix", "path-info", "-S", "--json", path], capture_output=True, text=True, check=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return None
    info = json.loads(out)
    entries = info.values() if isinstance(info, dict) else info
    return next(iter(entries))["closureSize"]


def summarize(runs):
    """Medians and ranges of the noisy fields; the model-side fields must agree."""
    ok = [r for r in runs if r["error"] is None]
    summary = {"runs": len(runs), "errors": [r["error"] for r in runs if r["error"]]}
    for field in NOISY:
        values = [r[field] for r in ok]
        summary[field] = {"median": statistics.median(values), "min": min(values), "max": max(values)} if values else None
    for field in EXACT:
        values = sorted({r[field] for r in ok})
        summary[field] = values[0] if len(values) == 1 else values
    return summary


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", action="append", required=True, metavar="LABEL=PATH")
    parser.add_argument("--lean-ctx", action="append", default=[], metavar="LABEL=PATH",
                        help="send LABEL's requests through the lean-ctx proxy at PATH")
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--scenario", action="append", help="run only these scenarios")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    binaries = {label: str(Path(path).resolve()) for label, path in (item.split("=", 1) for item in args.binary)}
    proxies = {label: str(Path(path).resolve()) for label, path in (item.split("=", 1) for item in args.lean_ctx)}
    if unknown := set(proxies) - set(binaries):
        parser.error(f"--lean-ctx names no --binary: {sorted(unknown)}")
    scenarios = tomllib.loads((HERE / "scenarios.toml").read_text())["scenario"]
    if args.scenario:
        scenarios = [s for s in scenarios if s["name"] in args.scenario]

    runs = {label: {s["name"]: [] for s in scenarios} for label in binaries}
    startups = {label: [] for label in binaries}
    for repetition in range(args.repeat):
        for label, binary in binaries.items():
            for _ in range(STARTUP_SAMPLES):
                startups[label].append(startup_ms(binary))
            for scenario in scenarios:
                result = run_once(binary, scenario, proxies.get(label))
                runs[label][scenario["name"]].append(result)
                print(f"[{repetition + 1}/{args.repeat}] {label} {scenario['name']}: "
                      f"{result['wall_ms']} ms {result['request_bytes']} B"
                      + (f" ERROR {result['error']}" if result["error"] else ""), file=sys.stderr)

    report = {
        "machine": {"system": platform.platform(), "cpus": os.cpu_count(), "load": os.getloadavg()},
        "repeat": args.repeat,
        "binaries": {
            label: {
                "path": binary,
                "lean_ctx": proxies.get(label),
                "size_bytes": executable_size(binary),
                "closure_bytes": closure_bytes(binary),
                "startup_ms": {"median": statistics.median(startups[label]),
                               "min": min(startups[label]), "max": max(startups[label])},
                "scenarios": {name: summarize(results) for name, results in runs[label].items()},
            }
            for label, binary in binaries.items()
        },
    }
    args.out.write_text(json.dumps(report, indent=2) + "\n")
    failed = sum(len(s["errors"]) for b in report["binaries"].values() for s in b["scenarios"].values())
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
