#!/usr/bin/env python3
"""Runs the fixed tasks in evals/tasks with `socium -p` and judges each run.

A run passes when the task's check passes and nothing outside the task's
allowed_changes was changed. The model's own claim of success counts for
nothing. Each task directory holds:

    task.toml   the request and how it is judged (below)
    repo/       copied into a fresh Git repository for each run
    dirty/      optional: written over repo/ after the commit, as uncommitted
                changes the user already had
    check.py    optional: a validator the agent never sees
    solution/   files a correct run would leave, for --self-check and --mock
    reply.txt   optional: a correct final reply, for checks that read it

task.toml has:

    category = "bug-fix"       # what the task measures, for the report
    difficulty = "hard"        # optional; "basic" when absent
    prompt = "..."             # one request; or
    turns = ["...", "..."]     # several, sent as one conversation (-p, then -c -p)
    check = ["argv", ...]      # run in the repository afterwards; exit 0 passes.
                               # {task} is replaced with the task directory.
    allowed_changes = ["glob"] # paths the task may change; anything else is unintended
    timeout = 600              # seconds for all turns (optional)
    max_tokens = 400000        # tokens per run before it is stopped (optional)

The check sees the agent's final reply in the file named by EVAL_REPLY_FILE.

Real runs call the configured model (MODEL, keys, models.toml) and need
--budget-usd: the model must have a price in models.toml, and no run starts
once the money spent plus the dearest run so far would pass the budget. A run
is stopped at its max_tokens, so the budget can be passed by at most one run.
SOCIUM_CONFIG_FILE defaults to evals/config.toml, which lets commands run
without asking.

--self-check judges each task's untouched repository (must fail) and its
solution (must pass, inside allowed_changes) without any model. --mock PLAY
runs the real binary against mock_provider.py instead of a model.

usage: run.py [--binary socium] [--only NAME ...] [--repeat N] [--out FILE]
              [--label NAME] [--comparison controlled|defaults] [--max-tokens N]
              (--budget-usd USD | --mock solution|nothing|stray | --self-check)
"""
import argparse
import datetime
import fnmatch
import hashlib
import json
import os
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
TASKS = HERE / "tasks"
RESULT_SCHEMA = 2
TASK_KEYS = {"category", "difficulty", "prompt", "turns", "check", "allowed_changes", "timeout", "max_tokens"}
DEFAULT_TIMEOUT = 600
DEFAULT_MAX_TOKENS = 400_000
CHECK_TIMEOUT = 600
# Files a run leaves that are not changes to the work: Socium's state and
# build or bytecode caches from running the tests.
IGNORED = [".socium/", "__pycache__/", "*.pyc", "target/", ".pytest_cache/"]
REPLY_FILE = "reply.txt"


def git(repo, *args):
    return subprocess.run(
        ["git", "-c", "user.name=eval", "-c", "user.email=eval@example.com", *args],
        cwd=repo,
        check=True,
        capture_output=True,
    ).stdout


def load_task(name):
    path = TASKS / name / "task.toml"
    task = tomllib.loads(path.read_text())
    unknown = set(task) - TASK_KEYS
    if unknown:
        raise SystemExit(f"{path}: unknown keys {sorted(unknown)}")
    if ("prompt" in task) == ("turns" in task):
        raise SystemExit(f"{path}: give either prompt or turns")
    for key in ("category", "check", "allowed_changes"):
        if key not in task:
            raise SystemExit(f"{path}: {key} is required")
    task["turns"] = task.pop("turns", None) or [task.pop("prompt")]
    task.setdefault("difficulty", "basic")
    task["check"] = [arg.replace("{task}", str(TASKS / name)) for arg in task["check"]]
    return task


def tree_digest(root):
    """A hash of every file under root, so a result names the exact task."""
    digest = hashlib.sha256()
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        digest.update(path.relative_to(root).as_posix().encode() + b"\0")
        digest.update(hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def file_digest(path):
    if not path or not Path(path).is_file():
        return None
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def overlay(source, target):
    for path in source.rglob("*"):
        if path.is_file():
            destination = target / path.relative_to(source)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, destination)


def prepare_repo(name, repo):
    """The task's repository with its commit and any uncommitted changes, and
    a snapshot of its files to diff the run against."""
    shutil.copytree(TASKS / name / "repo", repo)
    git(repo, "init", "-q")
    (repo / ".git" / "info").mkdir(exist_ok=True)
    (repo / ".git" / "info" / "exclude").write_text("\n".join(IGNORED) + "\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "task")
    if (TASKS / name / "dirty").is_dir():
        overlay(TASKS / name / "dirty", repo)
    return snapshot(repo)


def snapshot(repo):
    listed = git(repo, "ls-files", "-z", "--cached", "--others", "--exclude-standard")
    files = {}
    for path in filter(None, listed.decode().split("\0")):
        full = repo / path
        if full.is_file():
            files[path] = full.read_bytes()
    return files


def changes(repo, before):
    """Paths added, removed, or changed since `before`."""
    after = snapshot(repo)
    return sorted(p for p in before.keys() | after.keys() if before.get(p) != after.get(p))


def unintended(changed, allowed):
    return [p for p in changed if not any(fnmatch.fnmatchcase(p, glob) for glob in allowed)]


def judge(task, repo, before, reply_text, env):
    reply = repo.parent / "reply-for-check.txt"
    reply.write_text(reply_text)
    try:
        check = subprocess.run(
            task["check"],
            cwd=repo,
            env={**env, "EVAL_REPLY_FILE": str(reply)},
            capture_output=True,
            text=True,
            timeout=CHECK_TIMEOUT,
        )
    except subprocess.TimeoutExpired:
        check = subprocess.CompletedProcess(task["check"], 1, "", f"the check took over {CHECK_TIMEOUT}s")
    changed = changes(repo, before)
    stray = unintended(changed, task["allowed_changes"])
    return {
        "check_passed": check.returncode == 0,
        "changed": changed,
        "unintended_changes": stray,
        "passed": check.returncode == 0 and not stray,
        "check_output": (check.stdout + check.stderr)[-2000:],
    }


def run_turn(argv, repo, env, deadline, token_cap, tokens_so_far):
    """One `socium -p`, read as it streams so a run past its token cap or
    deadline is cancelled (SIGINT, as Ctrl+C) rather than left to spend."""
    agent = subprocess.Popen(
        argv, cwd=repo, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    records, stopped, tokens = [], None, tokens_so_far
    stderr = []
    reader = threading.Thread(target=lambda: stderr.append(agent.stderr.read()), daemon=True)
    reader.start()
    timer = threading.Timer(max(0.0, deadline - time.monotonic()), lambda: stop("timeout"))

    def stop(reason):
        nonlocal stopped
        if stopped is not None:
            return
        stopped = reason
        if agent.poll() is None:
            agent.send_signal(signal.SIGINT)
            threading.Timer(10, lambda: agent.poll() is None and agent.kill()).start()

    timer.start()
    for line in agent.stdout:
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        records.append(record)
        event = record.get("event") or {}
        if record.get("type") == "activity" and event.get("type") == "round_finished":
            usage = event.get("usage") or {}
            tokens += sum(usage.get(k, 0) for k in (
                "input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
            if tokens > token_cap:
                stop("token_cap")
    agent.wait()
    timer.cancel()
    reader.join(timeout=5)
    return records, agent.returncode, stopped, tokens, "".join(stderr)


def measure(records):
    activity = [r["event"] for r in records if r.get("type") == "activity"]
    finished = [r for r in records if r.get("type") == "finished"]
    usage = {}
    costs = []
    for record in finished:
        for key, value in (record.get("usage") or {}).items():
            usage[key] = usage.get(key, 0) + value
        costs.append(record.get("cost_usd"))
    tool_errors = sum(1 for e in activity if e.get("type") == "tool_finished" and e.get("is_error"))
    return {
        "rounds": sum(1 for e in activity if e.get("type") == "round_finished" and not e.get("agent")),
        "tools": [e.get("name") for e in activity if e.get("type") == "tool_finished"],
        "tool_errors": tool_errors,
        "provider_retries": sum(
            1 for e in activity if e.get("type") == "warning" and "retrying in" in e.get("message", "")
        ),
        "approvals_refused": sum(
            1 for e in activity if e.get("type") == "approval_resolved" and not e.get("approved")
        ),
        "usage": usage,
        # None when any turn's cost was unknown (a model without a price).
        "cost_usd": sum(costs) if costs and all(c is not None for c in costs) else None,
        "unreported_calls": sum(r.get("unreported_calls") or 0 for r in finished),
        "errors": [r.get("error") for r in finished if r.get("error")],
        "reply": (finished[-1].get("text") or "") if finished else "",
        "model": next((r.get("model_id") for r in records if r.get("type") == "started"), None),
    }


def run_task(name, task, binary, workdir, env):
    repo = workdir / "repo"
    before = prepare_repo(name, repo)
    deadline = time.monotonic() + task.get("timeout", DEFAULT_TIMEOUT)
    cap = task.get("max_tokens", DEFAULT_MAX_TOKENS)
    started = time.monotonic()
    records, statuses, stopped, tokens, stderr = [], [], None, 0, ""
    for index, prompt in enumerate(task["turns"]):
        argv = [binary, *(["-c"] if index else []), "-p", prompt]
        turn, status, stopped, tokens, err = run_turn(argv, repo, env, deadline, cap, tokens)
        records += turn
        statuses.append(status)
        stderr += err
        if stopped or status != 0:
            break
    seconds = round(time.monotonic() - started, 1)
    metrics = measure(records)
    verdict = judge(task, repo, before, metrics.pop("reply"), env)
    return {
        **verdict,
        "turns_sent": len(statuses),
        "exit_statuses": statuses,
        "stopped": stopped,
        "seconds": seconds,
        # -p has no one to ask: work that needs a person shows as refused approvals.
        "interventions": 0,
        # Tool errors or provider retries that the run still passed despite.
        "recovered": verdict["passed"] and (metrics["tool_errors"] > 0 or metrics["provider_retries"] > 0),
        **metrics,
        "stderr": stderr[-2000:],
    }


def mock_env(name, task, workdir, play):
    """An environment with nothing from the user's: a loopback model that
    plays the task's solution, priced so the cost path runs."""
    port_file = workdir / "port"
    server = subprocess.Popen([
        sys.executable, "-I", str(HERE / "mock_provider.py"),
        "--task", str(TASKS / name), "--repo", str(workdir / "repo"),
        "--turns", str(len(task["turns"])), "--play", play, "--port-file", str(port_file),
    ])
    for _ in range(100):
        if port_file.exists() and port_file.read_text():
            break
        time.sleep(0.05)
    models = workdir / "models.toml"
    models.write_text(
        "[providers.mock]\n"
        'wire = "openai-chat"\n'
        f'base_url = "http://127.0.0.1:{port_file.read_text()}/v1"\n'
        "[[models]]\n"
        'alias = "mock"\nprovider = "mock"\nid = "mock-model"\n'
        "context_window = 200000\nmax_output_tokens = 8000\n"
        "price = { input = 1.0, output = 2.0 }\n"
    )
    home = workdir / "home"
    home.mkdir()
    env = {
        "PATH": os.environ.get("PATH", ""),
        "HOME": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "SOCIUM_MODELS_FILE": str(models),
        "SOCIUM_CONFIG_FILE": str(HERE / "config.toml"),
        "MODEL": "mock",
    }
    return env, server


def manifest(args, names):
    """What a rerun must hold fixed to repeat this one."""
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=HERE, capture_output=True, text=True, check=True
        ).stdout.strip()
        dirty = bool(subprocess.run(
            ["git", "status", "--porcelain", "--", "crates", "evals"], cwd=HERE.parent,
            capture_output=True, text=True,
        ).stdout.strip())
    except (OSError, subprocess.CalledProcessError):
        commit, dirty = None, None
    binary = args.binary
    config = os.environ.get("SOCIUM_CONFIG_FILE", str(HERE / "config.toml"))
    models = os.environ.get("SOCIUM_MODELS_FILE") or str(
        Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "socium" / "models.toml")
    return {
        "type": "manifest",
        "schema": RESULT_SCHEMA,
        "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "label": args.label,
        "comparison": args.comparison,
        "mode": f"mock:{args.mock}" if args.mock else "model",
        "model": "mock" if args.mock else os.environ.get("MODEL"),
        "harness": "socium",
        "harness_commit": commit,
        "harness_dirty": dirty,
        "binary_sha256": file_digest(binary),
        "config_sha256": file_digest(HERE / "config.toml" if args.mock else config),
        "models_sha256": None if args.mock else file_digest(models),
        "budget_usd": args.budget_usd,
        "max_tokens": args.max_tokens,
        "repeat": args.repeat,
        # Socium sends no sampling seed; repetitions stand in for one.
        "seed": None,
        "tasks": {name: tree_digest(TASKS / name) for name in names},
        "runner_sha256": file_digest(Path(__file__)),
        "python": platform.python_version(),
        "platform": platform.platform(),
    }


def self_check(names):
    """Each check must fail on the untouched repository and pass on the
    solution without unintended changes; otherwise it cannot judge runs."""
    failures = 0
    for name in names:
        task = load_task(name)
        problems = []
        solution = TASKS / name / "solution"
        reply = TASKS / name / REPLY_FILE
        if not solution.is_dir() and not reply.exists():
            problems.append("neither solution/ nor reply.txt")
        with tempfile.TemporaryDirectory(prefix=f"socium-check-{name}-") as workdir:
            repo = Path(workdir) / "repo"
            before = prepare_repo(name, repo)
            if judge(task, repo, before, "", os.environ)["check_passed"]:
                problems.append("the untouched repository passes")
            if solution.is_dir():
                overlay(solution, repo)
            verdict = judge(task, repo, before, reply.read_text() if reply.exists() else "", os.environ)
            if not verdict["check_passed"]:
                problems.append("the solution fails: " + verdict["check_output"].strip()[-500:])
            if verdict["unintended_changes"]:
                problems.append(f"the solution changes {verdict['unintended_changes']}")
            if not verdict["changed"] and not reply.exists():
                problems.append("the solution changes nothing")
        failures += bool(problems)
        print(f"{'FAIL' if problems else 'ok'} {name}" + "".join(f"\n  {p}" for p in problems))
    print(f"{len(names) - failures}/{len(names)} tasks judge correctly")
    return 1 if failures else 0


def main():
    parser = argparse.ArgumentParser(usage=__doc__.rsplit("usage: ", 1)[1])
    parser.add_argument("--binary", default="socium")
    parser.add_argument("--only", nargs="*")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--out", default="results.jsonl")
    parser.add_argument("--label", default=None, help="a name for this side of a comparison")
    parser.add_argument(
        "--comparison", choices=["controlled", "defaults"], default="controlled",
        help="controlled: same model, endpoint, and budget as the other side; "
        "defaults: each harness as it ships",
    )
    parser.add_argument("--budget-usd", type=float, help="required for runs against a real model")
    parser.add_argument("--max-tokens", type=int, help="tokens per run, in place of each task's max_tokens")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--mock", choices=["solution", "nothing", "stray"])
    mode.add_argument("--self-check", action="store_true")
    args = parser.parse_args()
    if not (args.mock or args.self_check) and args.budget_usd is None:
        parser.error("a run against a real model needs --budget-usd")

    names = sorted(p.name for p in TASKS.iterdir() if (p / "task.toml").is_file())
    if args.only:
        missing = set(args.only) - set(names)
        if missing:
            raise SystemExit(f"no such task: {sorted(missing)}")
        names = [n for n in names if n in args.only]
    if args.self_check:
        return self_check(names)
    # The agent runs in each task's repository, so a relative path would miss.
    found = shutil.which(args.binary)
    if not found:
        raise SystemExit(f"no socium binary at {args.binary}")
    args.binary = str(Path(found).resolve())
    if args.budget_usd is not None and args.budget_usd <= 0:
        raise SystemExit("--budget-usd must be above zero")

    spent, dearest, results, halted = 0.0, 0.0, [], None
    with open(args.out, "w") as out:
        out.write(json.dumps(manifest(args, names)) + "\n")
        for repetition in range(args.repeat):
            for name in names:
                if args.budget_usd is not None and spent + dearest > args.budget_usd:
                    halted = f"budget: ${spent:.4f} spent, the dearest run cost ${dearest:.4f}"
                    break
                task = load_task(name)
                if args.max_tokens:
                    task["max_tokens"] = args.max_tokens
                with tempfile.TemporaryDirectory(prefix=f"socium-eval-{name}-") as workdir:
                    workdir = Path(workdir)
                    server = None
                    if args.mock:
                        env, server = mock_env(name, task, workdir, args.mock)
                    else:
                        env = dict(os.environ)
                        env.setdefault("SOCIUM_CONFIG_FILE", str(HERE / "config.toml"))
                    try:
                        result = run_task(name, task, args.binary, workdir, env)
                    finally:
                        if server:
                            server.terminate()
                            server.wait()
                result = {"type": "result", "task": name, "category": task["category"],
                          "difficulty": task["difficulty"],
                          "repetition": repetition, **result}
                results.append(result)
                out.write(json.dumps(result) + "\n")
                out.flush()
                cost = result["cost_usd"]
                if args.budget_usd is not None and (cost is None or result["unreported_calls"]):
                    halted = ("budget: the run's cost is unknown (a model without a price in "
                              "models.toml, or calls that reported no usage)")
                    break
                spent += cost or 0.0
                dearest = max(dearest, cost or 0.0)
                mark = "PASS" if result["passed"] else "FAIL"
                extra = f", unintended: {result['unintended_changes']}" if result["unintended_changes"] else ""
                print(f"{mark} {name} #{repetition} ({result['seconds']}s, {result['rounds']} rounds{extra})")
            if halted:
                break
        if halted:
            out.write(json.dumps({"type": "halted", "reason": halted}) + "\n")
            print(f"stopped: {halted}")
    for difficulty in sorted({r["difficulty"] for r in results}):
        group = [r for r in results if r["difficulty"] == difficulty]
        print(f"{difficulty}: {sum(r['passed'] for r in group)}/{len(group)} passed")
    passed = sum(r["passed"] for r in results)
    planned = len(names) * args.repeat
    print(f"{passed}/{len(results)} passed ({planned - len(results)} not run); details in {args.out}")
    return 0 if passed == len(results) == planned else 1


if __name__ == "__main__":
    sys.exit(main())
