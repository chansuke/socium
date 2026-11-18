import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).parent
sys.path.insert(0, ".")
import calc

assert calc.half(5) == 2.5
assert calc.triple(2) == 6, "the uncommitted triple() is gone"
assert "# work in progress" in open("calc.py").read()
assert open("notes.md").read() == (HERE / "dirty" / "notes.md").read_text(), "notes.md changed"
git = lambda *a: subprocess.run(["git", *a], capture_output=True, text=True).stdout
assert git("rev-list", "--count", "HEAD").strip() == "1", "a commit was made"
assert git("stash", "list").strip() == "", "work was stashed"
