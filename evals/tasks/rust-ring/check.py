import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).parent
with tempfile.TemporaryDirectory() as tmp:
    copy = pathlib.Path(tmp) / "repo"
    shutil.copytree(".", copy, ignore=shutil.ignore_patterns("target", ".git", ".socium"))
    (copy / "tests").mkdir(exist_ok=True)
    shutil.copy(HERE / "hidden.rs", copy / "tests" / "hidden_contract.rs")
    env = {**os.environ, "CARGO_TARGET_DIR": str(pathlib.Path(tmp) / "target")}
    run = subprocess.run(["cargo", "test", "--offline", "--quiet"], cwd=copy,
                         capture_output=True, text=True, env=env)
    print(run.stdout[-3000:], run.stderr[-3000:])
    if run.returncode:
        sys.exit(1)
assert "#[test]" in "".join(p.read_text() for p in pathlib.Path(".").rglob("*.rs") if "target" not in p.parts), "no tests were added"
