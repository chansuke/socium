#!/usr/bin/env python3
"""Compare Socium with other coding agents on one short prompt.

Every agent gets the same fixture repository, an empty HOME, and the loopback
model from mock_model.py on its own wire, and is asked "Say hello." once in
its headless mode. The runner records what the agent sent the model (POST
requests and bytes, which is mostly its system prompt and tool definitions),
how long the run and `--version` took, and the peak memory of the process
tree. Agents are run in turn within each repetition.

The agents are not doing equal work: some send extra requests (titles,
token counts) or start helpers. Those show up in the request counts, and the
numbers say what each agent costs to start and to ask once, not how well it
codes.

usage: agents.py AGENT=PATH [AGENT=PATH ...] [--repeat N] --out FILE
       AGENT is one of: socium claude-code codex opencode crush pi gemini-cli goose
"""
import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run import closure_bytes, prepare, start_model, write_models  # noqa: E402

PROMPT = "Say hello."
FIXTURE = {"repo": "evals/tasks/cite-location/repo", "calls": []}
TIMEOUT_SECONDS = 180
KEY = "sk-loopback-not-a-real-key"


def socium(port, home):
    return write_models(home / "models.toml", port), ["-p", PROMPT]


def claude_code(port, home):
    return {
        "ANTHROPIC_BASE_URL": f"http://127.0.0.1:{port}",
        "ANTHROPIC_API_KEY": KEY,
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        "DISABLE_AUTOUPDATER": "1",
        "DISABLE_TELEMETRY": "1",
    }, ["-p", PROMPT]


def codex(port, home):
    codex_home = home / ".codex"
    codex_home.mkdir()
    (codex_home / "config.toml").write_text(
        'model = "mock-model"\nmodel_provider = "mock"\n'
        "[model_providers.mock]\n"
        f'name = "mock"\nbase_url = "http://127.0.0.1:{port}/v1"\nwire_api = "responses"\n'
        'env_key = "MOCK_API_KEY"\nsupports_websockets = false\n'
    )
    return {"CODEX_HOME": str(codex_home), "MOCK_API_KEY": KEY}, ["exec", "--skip-git-repo-check", PROMPT]


def opencode(port, home):
    config = home / "opencode.json"
    config.write_text(json.dumps({
        "$schema": "https://opencode.ai/config.json",
        "provider": {"mock": {
            "npm": "@ai-sdk/openai-compatible",
            "options": {"baseURL": f"http://127.0.0.1:{port}/v1", "apiKey": KEY},
            "models": {"mock-model": {}},
        }},
        "model": "mock/mock-model",
        "small_model": "mock/mock-model",
        "autoupdate": False,
        "share": "disabled",
    }))
    return {"OPENCODE_CONFIG": str(config), "OPENCODE_DISABLE_AUTOUPDATE": "1"}, ["run", PROMPT]


def crush(port, home):
    config = home / ".config" / "crush"
    config.mkdir(parents=True)
    model = {"id": "mock-model", "name": "mock", "context_window": 200000, "default_max_tokens": 8000}
    (config / "crush.json").write_text(json.dumps({
        "providers": {"mock": {"type": "openai-compat", "base_url": f"http://127.0.0.1:{port}/v1",
                               "api_key": KEY, "models": [model]}},
        "models": {"large": {"model": "mock-model", "provider": "mock"},
                   "small": {"model": "mock-model", "provider": "mock"}},
        "options": {"disable_provider_auto_update": True, "disable_metrics": True},
    }))
    # Without --quiet the reply is sometimes missing from a redirected stdout.
    return {"CRUSH_DISABLE_METRICS": "1", "CRUSH_DISABLE_PROVIDER_AUTO_UPDATE": "1"}, ["run", "--quiet", PROMPT]


def pi(port, home):
    agent = home / ".pi" / "agent"
    agent.mkdir(parents=True)
    (agent / "models.json").write_text(json.dumps({"providers": {"mock": {
        "baseUrl": f"http://127.0.0.1:{port}/v1", "api": "openai-completions", "apiKey": KEY,
        "models": [{"id": "mock-model", "name": "mock", "contextWindow": 200000, "maxTokens": 8000}],
    }}}))
    return {}, ["-p", "--provider", "mock", "--model", "mock-model", PROMPT]


def gemini_cli(port, home):
    settings = home / ".gemini"
    settings.mkdir()
    (settings / "settings.json").write_text(json.dumps({
        "security": {"auth": {"selectedType": "gemini-api-key"}},
        "privacy": {"usageStatisticsEnabled": False},
    }))
    return {"GEMINI_API_KEY": KEY, "GOOGLE_GEMINI_BASE_URL": f"http://127.0.0.1:{port}",
            "GEMINI_CLI_TRUST_WORKSPACE": "true"}, ["-p", PROMPT]


def goose(port, home):
    return {
        "GOOSE_PROVIDER": "openai", "GOOSE_MODEL": "mock-model", "OPENAI_HOST": f"http://127.0.0.1:{port}",
        "OPENAI_API_KEY": KEY, "GOOSE_DISABLE_KEYRING": "1", "GOOSE_TELEMETRY_ENABLED": "false",
    }, ["run", "--no-session", "-t", PROMPT]


AGENTS = {
    "socium": socium, "claude-code": claude_code, "codex": codex, "opencode": opencode,
    "crush": crush, "pi": pi, "gemini-cli": gemini_cli, "goose": goose,
}


def ask_once(agent, binary):
    with tempfile.TemporaryDirectory(prefix="agent-bench-") as tmp:
        workdir = Path(tmp)
        repo, home = prepare(FIXTURE, workdir)
        server, port = start_model(workdir)
        extra, args = AGENTS[agent](port, home)
        env = {"PATH": os.environ.get("PATH", ""), "HOME": str(home),
               "XDG_CONFIG_HOME": str(home / ".config"), "TERM": "dumb", **extra}
        output = workdir / "output"
        try:
            with output.open("wb") as sink:
                started = time.perf_counter()
                process = subprocess.Popen([binary, *args], cwd=repo, env=env, stdin=subprocess.DEVNULL,
                                           stdout=sink, stderr=subprocess.STDOUT, start_new_session=True)
                timer = threading.Timer(TIMEOUT_SECONDS, lambda: os.killpg(process.pid, 9))
                timer.start()
                _, status, usage = os.wait4(process.pid, 0)
                wall = time.perf_counter() - started
                timer.cancel()
        finally:
            server.kill()
            server.wait()
        log = workdir / "requests.jsonl"
        requests = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
        text = output.read_bytes().decode(errors="replace")
    code = os.waitstatus_to_exitcode(status)
    error = None
    if code != 0:
        error = f"exit {code}: {text[-300:]}"
    elif not requests:
        error = "the agent never called the model"
    elif "Done." not in text:
        error = f"the reply never reached the output: {text[-300:]}"
    return {
        "wall_ms": round(wall * 1000, 1),
        "cpu_ms": round((usage.ru_utime + usage.ru_stime) * 1000, 1),
        "max_rss_kib": usage.ru_maxrss,
        "requests": len(requests),
        "request_bytes": sum(r["bytes"] for r in requests),
        "largest_request_bytes": max((r["bytes"] for r in requests), default=0),
        "paths": sorted({r["path"] for r in requests}),
        "error": error,
    }


def version_ms(binary):
    with tempfile.TemporaryDirectory(prefix="agent-bench-") as home:
        started = time.perf_counter()
        # Socium has no --version; --help does as little.
        flag = "--help" if Path(binary).name == "socium" else "--version"
        subprocess.run([binary, flag], capture_output=True, timeout=60,
                       env={"PATH": os.environ.get("PATH", ""), "HOME": home})
        return round((time.perf_counter() - started) * 1000, 1)


def spread(values):
    return {"median": statistics.median(values), "min": min(values), "max": max(values)} if values else None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("agents", nargs="+", metavar="AGENT=PATH")
    parser.add_argument("--repeat", type=int, default=5)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    binaries = {}
    for item in args.agents:
        name, path = item.split("=", 1)
        if name not in AGENTS:
            parser.error(f"unknown agent {name}")
        binaries[name] = str(Path(shutil.which(path) or path).resolve())

    runs = {name: [] for name in binaries}
    versions = {name: [] for name in binaries}
    for repetition in range(args.repeat):
        for name, binary in binaries.items():
            versions[name].append(version_ms(binary))
            result = ask_once(name, binary)
            runs[name].append(result)
            print(f"[{repetition + 1}/{args.repeat}] {name}: {result['wall_ms']} ms, "
                  f"{result['requests']} requests, {result['request_bytes']} B"
                  + (f" ERROR {result['error']}" if result["error"] else ""), file=sys.stderr)

    report = {"machine": {"cpus": os.cpu_count(), "load": os.getloadavg()}, "repeat": args.repeat, "agents": {}}
    for name, results in runs.items():
        ok = [r for r in results if r["error"] is None]
        report["agents"][name] = {
            "path": binaries[name],
            "closure_bytes": closure_bytes(binaries[name]),
            "version_ms": spread(versions[name]),
            **{field: spread([r[field] for r in ok]) for field in ("wall_ms", "cpu_ms", "max_rss_kib")},
            **{field: sorted({r[field] for r in ok}) for field in ("requests", "request_bytes", "largest_request_bytes")},
            "paths": sorted({p for r in ok for p in r["paths"]}),
            "errors": [r["error"] for r in results if r["error"]],
        }
    args.out.write_text(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
