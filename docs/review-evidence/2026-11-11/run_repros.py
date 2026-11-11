#!/usr/bin/env python3
"""Run review-only bug observations in a temporary source copy.

These tests PASS when the reviewed defect is present; they are not acceptance
tests for corrected behavior. No API credentials or network requests are used
by the tests. Cargo uses its normal dependency cache; --offline is required.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    evidence = Path(__file__).resolve().parent
    repository = evidence.parents[2]
    with tempfile.TemporaryDirectory(prefix="socium-review-") as temporary:
        source = Path(temporary)
        for name in ("Cargo.toml", "Cargo.lock"):
            shutil.copy2(repository / name, source / name)
        shutil.copytree(repository / "crates", source / "crates")
        core = source / "crates/core/src"
        shutil.copy2(evidence / "review_repros.rs", core / "review_repros.rs")
        with (core / "lib.rs").open("a") as output:
            output.write('\n#[cfg(test)]\nmod review_repros;\n')
        with (core / "providers/anthropic/stream.rs").open("a") as output:
            output.write('\n' + (evidence / "stream_repros.rs").read_text())
        environment = os.environ.copy()
        environment.setdefault("CARGO_TARGET_DIR", str(source / "target"))
        result = subprocess.run(
            ["cargo", "test", "-p", "socium-core", "--locked", "--offline",
             "review_", "--", "--nocapture"],
            cwd=source, env=environment, check=False,
        )
        return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
