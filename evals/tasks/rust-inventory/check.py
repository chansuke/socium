import pathlib
import shutil
import subprocess
import tempfile

HERE = pathlib.Path(__file__).parent
with tempfile.TemporaryDirectory() as tmp:
    copy = pathlib.Path(tmp) / "crate"
    shutil.copytree(".", copy, ignore=shutil.ignore_patterns("target", ".git", ".socium"))
    (copy / "tests").mkdir(exist_ok=True)
    shutil.copyfile(HERE / "hidden.rs", copy / "tests" / "hidden_eval.rs")
    run = subprocess.run(["cargo", "test", "--offline", "--quiet"], cwd=copy, capture_output=True, text=True)
    print(run.stdout[-3000:], run.stderr[-3000:])
    raise SystemExit(run.returncode)
