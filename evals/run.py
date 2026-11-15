#!/usr/bin/env python3
"""Runs each task in evals/tasks with `socium-tui -p` and checks the result.

Each task directory holds `task.toml` and a `repo/` that is copied into a
fresh Git repository for the run. `task.toml` has:

    prompt = "..."           # the request, as a user would type it
    check = ["argv", ...]    # run in the repository afterwards; exit 0 passes
    timeout = 600            # seconds for the agent (optional)

The check sees the agent's final reply in the file named by EVAL_REPLY_FILE.
Results go to a JSON Lines file, one line per task, and a summary is printed.

Every run calls the model you configured, so it costs money. Socium's own
environment applies (MODEL, keys, models.toml); SOCIUM_CONFIG_FILE defaults to
evals/config.toml, which lets commands run without asking.

usage: run.py [--binary socium-tui] [--only NAME ...] [--out results.jsonl]
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent


def git(repo, *args):
    subprocess.run(
        ["git", "-c", "user.name=eval", "-c", "user.email=eval@example.com", *args],
        cwd=repo,
        check=True,
        capture_output=True,
    )


def run_task(name, task, binary, workdir):
    repo = workdir / "repo"
    shutil.copytree(HERE / "tasks" / name / "repo", repo)
    git(repo, "init", "-q")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "task")
    env = dict(os.environ)
    env.setdefault("SOCIUM_CONFIG_FILE", str(HERE / "config.toml"))
    started = time.monotonic()
    try:
        agent = subprocess.run(
            [binary, "-p", task["prompt"]],
            cwd=repo,
            env=env,
            capture_output=True,
            text=True,
            timeout=task.get("timeout", 600),
        )
        records = [json.loads(line) for line in agent.stdout.splitlines() if line.strip()]
        status, stderr = agent.returncode, agent.stderr
    except subprocess.TimeoutExpired:
        records, status, stderr = [], None, "timed out"
    seconds = round(time.monotonic() - started, 1)
    finished = next((r for r in reversed(records) if r.get("type") == "finished"), {})
    reply = workdir / "reply.txt"
    reply.write_text(finished.get("text") or "")
    check = subprocess.run(
        task["check"],
        cwd=repo,
        env={**env, "EVAL_REPLY_FILE": str(reply)},
        capture_output=True,
        text=True,
    )
    rounds = sum(
        1
        for r in records
        if r.get("type") == "activity"
        and r["event"].get("type") == "round_finished"
        and not r["event"].get("agent")
    )
    tools = [
        r["event"].get("name")
        for r in records
        if r.get("type") == "activity" and r["event"].get("type") == "tool_finished"
    ]
    return {
        "task": name,
        "passed": check.returncode == 0,
        "exit_status": status,
        "seconds": seconds,
        "rounds": rounds,
        "tools": tools,
        "usage": finished.get("usage"),
        "error": finished.get("error"),
        "stderr": stderr[-2000:],
        "check_output": (check.stdout + check.stderr)[-2000:],
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="socium-tui")
    parser.add_argument("--only", nargs="*")
    parser.add_argument("--out", default="results.jsonl")
    args = parser.parse_args()
    names = sorted(p.name for p in (HERE / "tasks").iterdir() if p.is_dir())
    if args.only:
        names = [n for n in names if n in args.only]
    results = []
    with open(args.out, "w") as out:
        for name in names:
            task = tomllib.loads((HERE / "tasks" / name / "task.toml").read_text())
            with tempfile.TemporaryDirectory(prefix=f"socium-eval-{name}-") as workdir:
                result = run_task(name, task, args.binary, Path(workdir))
            results.append(result)
            out.write(json.dumps(result) + "\n")
            out.flush()
            mark = "PASS" if result["passed"] else "FAIL"
            print(f"{mark} {name} ({result['seconds']}s, {result['rounds']} rounds)")
    passed = sum(r["passed"] for r in results)
    print(f"{passed}/{len(results)} passed; details in {args.out}")
    return 0 if passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
