"""Checks that the evaluation judges runs correctly, with no paid model.

    nix develop -c python3 -m unittest evals/test_evals.py

The runs go through the real binary (SOCIUM_BINARY, or target/debug/socium)
against mock_provider.py; build it first with `cargo build`.
"""
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import compare  # noqa: E402

BINARY = Path(os.environ.get("SOCIUM_BINARY", HERE.parent / "target" / "debug" / "socium"))


def run(*args):
    """run.py's records and output."""
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "results.jsonl"
        done = subprocess.run(
            [sys.executable, str(HERE / "run.py"), "--binary", str(BINARY), "--out", str(out), *args],
            capture_output=True, text=True, timeout=600,
        )
        records = [json.loads(line) for line in out.read_text().splitlines()] if out.exists() else []
    return records, done


def results(records):
    return [r for r in records if r["type"] == "result"]


class SelfCheck(unittest.TestCase):
    def test_every_check_fails_untouched_and_passes_its_solution(self):
        _, done = run("--self-check")
        self.assertEqual(done.returncode, 0, done.stdout)


@unittest.skipUnless(BINARY.is_file(), f"no binary at {BINARY}")
class MockRuns(unittest.TestCase):
    def test_the_solution_passes_every_task(self):
        records, done = run("--mock", "solution")
        self.assertEqual(done.returncode, 0, done.stdout)
        runs = results(records)
        self.assertEqual(len(runs), len(list(HERE.glob("tasks/*/task.toml"))))
        for r in runs:
            self.assertTrue(r["passed"], r["task"])
            self.assertGreater(r["usage"]["input_tokens"], 0)
            self.assertGreater(r["cost_usd"], 0)
            self.assertEqual(r["exit_statuses"], [0] * r["turns_sent"])
        manifest = records[0]
        self.assertEqual(manifest["type"], "manifest")
        self.assertEqual(set(manifest["tasks"]), {r["task"] for r in runs})
        self.assertIsNotNone(manifest["binary_sha256"])

    def test_a_claim_of_success_without_the_work_fails(self):
        records, done = run("--mock", "nothing")
        self.assertEqual(done.returncode, 1)
        self.assertEqual([r["task"] for r in results(records) if r["passed"]], [])

    def test_a_change_outside_the_task_fails_even_when_the_check_passes(self):
        records, _ = run("--mock", "stray", "--only", "crlf-edit", "fix-chunk")
        for r in results(records):
            self.assertTrue(r["check_passed"], r["task"])
            self.assertFalse(r["passed"])
            self.assertEqual(r["unintended_changes"], ["notes-from-the-agent.txt"])

    def test_turns_are_one_conversation(self):
        records, _ = run("--mock", "solution", "--only", "narrowed-scope")
        (r,) = results(records)
        self.assertEqual(r["turns_sent"], 2)
        self.assertTrue(r["passed"])

    def test_no_run_starts_once_the_budget_would_be_passed(self):
        # A mock run of crlf-edit costs about $0.007, so a second would pass $0.01.
        records, done = run("--mock", "solution", "--repeat", "3", "--budget-usd", "0.01",
                            "--only", "crlf-edit")
        self.assertEqual(done.returncode, 1)
        runs = results(records)
        self.assertEqual(len(runs), 1)
        self.assertLess(runs[0]["cost_usd"], 0.01)
        self.assertTrue(records[-1]["reason"].startswith("budget"), records[-1])
        self.assertIn("not run", done.stdout)

    def test_a_run_past_its_cost_cap_is_stopped(self):
        # The first round alone costs more than this.
        records, _ = run("--mock", "solution", "--max-cost-usd", "0.0001", "--only", "conversation-accumulate")
        (r,) = results(records)
        self.assertEqual(r["stopped"], "cost_cap")
        self.assertEqual(r["turns_sent"], 1)
        self.assertFalse(r["passed"])

    def test_a_real_model_run_needs_a_budget(self):
        _, done = run("--only", "crlf-edit")
        self.assertEqual(done.returncode, 2)
        self.assertIn("--budget-usd", done.stderr)


class CostCap(unittest.TestCase):
    def test_cache_reads_cost_their_own_price(self):
        import run as runner

        usage = {"input_tokens": 1_000_000, "output_tokens": 1_000_000,
                 "cache_read_input_tokens": 1_000_000, "cache_creation_input_tokens": 1_000_000}
        cost = runner.round_cost(usage, {"input": 0.3, "output": 1.2, "cache_read": 0.006})
        self.assertAlmostEqual(cost, 0.3 + 1.2 + 0.006 + 0.3)

    def test_a_model_without_a_price_has_none(self):
        import run as runner

        with tempfile.TemporaryDirectory() as tmp:
            models = Path(tmp) / "models.toml"
            models.write_text('[[models]]\nalias = "a"\nprice = { input = 1.0 }\n[[models]]\nalias = "b"\n')
            env = {"SOCIUM_MODELS_FILE": str(models)}
            self.assertEqual(runner.price_of(env, "a"), {"input": 1.0})
            self.assertIsNone(runner.price_of(env, "b"))
            self.assertIsNone(runner.price_of(env, "missing"))
            self.assertIsNone(runner.price_of({"SOCIUM_MODELS_FILE": str(Path(tmp) / "none")}, "a"))


class TaskDigest(unittest.TestCase):
    def test_bytecode_from_running_a_check_does_not_change_the_task(self):
        import run as runner

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "check.py").write_text("print(1)\n")
            before = runner.tree_digest(root)
            (root / "__pycache__").mkdir()
            (root / "__pycache__" / "check.cpython-313.pyc").write_bytes(b"mtime")
            self.assertEqual(runner.tree_digest(root), before)
            (root / "check.py").write_text("print(2)\n")
            self.assertNotEqual(runner.tree_digest(root), before)


class ToolErrors(unittest.TestCase):
    def test_only_calls_that_could_not_run_as_sent_are_argument_errors(self):
        import run as runner

        def finished(summary, is_error=True):
            return {"type": "activity", "event": {"type": "tool_finished", "is_error": is_error,
                                                  "summary": summary}}

        metrics = runner.measure([
            finished("Unsupported tool: git_log"),
            finished("The arguments of this read_file call were not valid JSON (EOF); nothing ran."),
            finished("read_file requires a string field: path"),
            finished("No match for old_str in src/lib.rs"),
            finished("Unsupported tool: git_log", is_error=False),
        ])
        self.assertEqual(metrics["tool_errors"], 4)
        self.assertEqual(metrics["tool_argument_errors"], 3)


def side(path, label, passes, model="m", task_digest="t", tools=None):
    with open(path, "w") as f:
        f.write(json.dumps({"type": "manifest", "label": label, "comparison": "controlled", "tools": tools,
                            "mode": "model", "model": model, "budget_usd": 1.0,
                            "tasks": {"a": task_digest, "b": "t"}}) + "\n")
        for passed in passes:
            f.write(json.dumps({"type": "result", "task": "a", "passed": passed,
                                "unintended_changes": [], "cost_usd": 0.1, "seconds": 1}) + "\n")


class Compare(unittest.TestCase):
    def lines(self, a, b, **kwargs):
        with tempfile.TemporaryDirectory() as tmp:
            side(f"{tmp}/a", "old", a)
            side(f"{tmp}/b", "new", b, **kwargs)
            text = io.StringIO()
            with redirect_stdout(text):
                compare.compare(f"{tmp}/a", f"{tmp}/b")
            return text.getvalue()

    def test_a_small_difference_is_not_called_better(self):
        out = self.lines([True, False, True], [True, True, True])
        self.assertIn("a: no clear difference", out)
        self.assertNotIn("better", out)

    def test_a_clear_difference_is_named(self):
        out = self.lines([False] * 12, [True] * 12)
        self.assertIn("a: new better", out)

    def test_a_different_model_is_not_a_controlled_comparison(self):
        with self.assertRaises(SystemExit) as err:
            self.lines([True], [True], model="other")
        self.assertIn("model", str(err.exception))

    def test_a_changed_task_is_refused(self):
        out = self.lines([True], [True], task_digest="changed")
        self.assertIn("a: refused", out)
        self.assertIn("nothing to compare", out)

    def test_wilson_bounds(self):
        low, high = compare.wilson(0, 5)
        self.assertEqual(low, 0.0)
        self.assertAlmostEqual(high, 0.4345, places=3)


if __name__ == "__main__":
    unittest.main()
