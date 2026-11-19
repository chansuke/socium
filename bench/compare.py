#!/usr/bin/env python3
"""Print a Markdown table comparing two binaries from one bench/run.py report.

Bytes and request counts are what the model received and do not vary between
runs, so any change is shown. Times and memory vary, so head is called
better or worse (lower is better for each) only when the ranges of the two
sides' runs do not overlap; otherwise the cell says "~".

usage: compare.py REPORT [--base LABEL] [--head LABEL]
"""
import argparse
import json
import sys
from pathlib import Path

NOISY = ("wall_ms", "cpu_ms", "max_rss_kib")


def exact_cell(base, head):
    if base == head:
        return f"{head}"
    if not isinstance(base, int) or not isinstance(head, int) or base == 0:
        return f"{base} → {head}"
    return f"{base} → {head} ({(head - base) / base:+.1%})"


def noisy_cell(base, head):
    if base is None or head is None:
        return "no runs"
    change = (head["median"] - base["median"]) / base["median"] if base["median"] else 0.0
    if head["min"] > base["max"]:
        verdict = "worse"
    elif head["max"] < base["min"]:
        verdict = "better"
    else:
        verdict = "~"
    return f"{base['median']:g} → {head['median']:g} ({change:+.1%}, {verdict})"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("report", type=Path)
    parser.add_argument("--base", default="base")
    parser.add_argument("--head", default="head")
    args = parser.parse_args()
    report = json.loads(args.report.read_text())
    base, head = report["binaries"][args.base], report["binaries"][args.head]

    lines = [
        f"{report['repeat']} runs per side, interleaved; ranges that overlap are shown as `~`.",
        "",
        "| | base → head |",
        "|---|---|",
        f"| binary bytes | {exact_cell(base['size_bytes'], head['size_bytes'])} |",
        f"| Nix closure bytes | {exact_cell(base.get('closure_bytes'), head.get('closure_bytes'))} |",
        f"| `--help` ms | {noisy_cell(base['startup_ms'], head['startup_ms'])} |",
        "",
        "| scenario | requests | bytes sent | wall ms | CPU ms | peak KiB |",
        "|---|---|---|---|---|---|",
    ]
    errors = []
    for name, head_summary in head["scenarios"].items():
        base_summary = base["scenarios"].get(name)
        if base_summary is None:
            lines.append(f"| {name} | head only | | | | |")
            continue
        cells = [exact_cell(base_summary[f], head_summary[f]) for f in ("requests", "request_bytes")]
        cells += [noisy_cell(base_summary[f], head_summary[f]) for f in NOISY]
        lines.append(f"| {name} | " + " | ".join(cells) + " |")
        for label, summary in ((args.base, base_summary), (args.head, head_summary)):
            errors += [f"{label} {name}: {e}" for e in summary["errors"]]
    if errors:
        lines += ["", "Runs that failed and are left out of the numbers:", ""]
        lines += [f"- {e}" for e in errors]
    print("\n".join(lines))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
