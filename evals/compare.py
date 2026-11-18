#!/usr/bin/env python3
"""Compares two run.py result files without claiming more than they show.

A side is only called better on a task, or overall, when the 95% Wilson
intervals of the two pass rates do not overlap; anything else is reported as
no clear difference, with the run counts. Tasks run on one side only are
listed as not compared, and tasks whose files differ between the runs are
refused. A controlled comparison must hold the model, budget, and token cap
fixed; a defaults comparison lets them differ and says so.

usage: compare.py A.jsonl B.jsonl
"""
import json
import math
import statistics
import sys

Z = 1.96
# What a controlled comparison must hold fixed between the two sides.
CONTROLLED = ("model", "budget_usd", "max_tokens", "config_sha256", "models_sha256")


def load(path):
    manifest, results, halted = None, [], []
    with open(path) as f:
        for line in f:
            record = json.loads(line)
            kind = record.get("type")
            if kind == "manifest":
                manifest = record
            elif kind == "result":
                results.append(record)
            elif kind == "halted":
                halted.append(record["reason"])
    if manifest is None:
        raise SystemExit(f"{path}: no manifest; rerun it with this run.py")
    return manifest, results, halted


def wilson(passed, runs):
    if runs == 0:
        return (0.0, 1.0)
    p = passed / runs
    centre = p + Z * Z / (2 * runs)
    spread = Z * math.sqrt(p * (1 - p) / runs + Z * Z / (4 * runs * runs))
    scale = 1 + Z * Z / runs
    return (max(0.0, (centre - spread) / scale), min(1.0, (centre + spread) / scale))


def verdict(a, b, names):
    """`names` is (A, B). a and b are (passed, runs)."""
    low_a, high_a = wilson(*a)
    low_b, high_b = wilson(*b)
    if low_a > high_b:
        return f"{names[0]} better"
    if low_b > high_a:
        return f"{names[1]} better"
    return "no clear difference"


def side_name(manifest, fallback):
    return manifest.get("label") or (manifest.get("harness_commit") or fallback)[:10]


def summary(results):
    def middle(values):
        values = [v for v in values if v is not None]
        return statistics.median(values) if values else None

    return {
        "passed": sum(r["passed"] for r in results),
        "runs": len(results),
        "unintended": sum(bool(r["unintended_changes"]) for r in results),
        "cost": middle([r["cost_usd"] for r in results]),
        "seconds": middle([r["seconds"] for r in results]),
    }


def describe(s):
    cost = "?" if s["cost"] is None else f"${s['cost']:.4f}"
    return (f"{s['passed']}/{s['runs']} passed, {s['unintended']} with unintended changes, "
            f"median {cost} and {s['seconds']}s")


def compare(path_a, path_b, out=print):
    (ma, ra, ha), (mb, rb, hb) = load(path_a), load(path_b)
    names = (side_name(ma, "A"), side_name(mb, "B"))
    if names[0] == names[1]:
        names = (f"A ({names[0]})", f"B ({names[1]})")
    if ma["comparison"] != mb["comparison"]:
        raise SystemExit(f"one side is a {ma['comparison']} run and the other {mb['comparison']}")
    differs = [key for key in CONTROLLED if ma.get(key) != mb.get(key)]
    if ma["comparison"] == "controlled" and differs:
        raise SystemExit(f"not a controlled comparison: {', '.join(differs)} differ")
    out(f"{ma['comparison']} comparison: {names[0]} ({ma['mode']}) vs {names[1]} ({mb['mode']})")
    if differs:
        out(f"  differs by design: {', '.join(differs)}")
    for name, halted in ((names[0], ha), (names[1], hb)):
        for reason in halted:
            out(f"  {name} stopped early: {reason}")

    tasks_a = {r["task"] for r in ra}
    tasks_b = {r["task"] for r in rb}
    changed = sorted(t for t in tasks_a & tasks_b if ma["tasks"].get(t) != mb["tasks"].get(t))
    shared = sorted((tasks_a & tasks_b) - set(changed))
    for task in changed:
        out(f"  {task}: refused, the task's files differ between the runs")
    for task in sorted(tasks_a ^ tasks_b):
        out(f"  {task}: not compared, run on {names[0] if task in tasks_a else names[1]} only")

    pooled = ([], [])
    for task in shared:
        sa = summary([r for r in ra if r["task"] == task])
        sb = summary([r for r in rb if r["task"] == task])
        pooled[0].extend(r for r in ra if r["task"] == task)
        pooled[1].extend(r for r in rb if r["task"] == task)
        out(f"  {task}: {verdict((sa['passed'], sa['runs']), (sb['passed'], sb['runs']), names)}")
        out(f"    {names[0]}: {describe(sa)}")
        out(f"    {names[1]}: {describe(sb)}")
    if not shared:
        out("no task was run on both sides; nothing to compare")
        return
    sa, sb = summary(pooled[0]), summary(pooled[1])
    out(f"overall on {len(shared)} shared tasks: "
        f"{verdict((sa['passed'], sa['runs']), (sb['passed'], sb['runs']), names)}")
    out(f"  {names[0]}: {describe(sa)}")
    out(f"  {names[1]}: {describe(sb)}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(__doc__.rsplit("usage: ", 1)[1])
    compare(sys.argv[1], sys.argv[2])
