"""The tests must pass on wrap.py and fail on each broken copy in mutants/."""
import pathlib
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).parent


def tests_pass(wrap_source):
    with tempfile.TemporaryDirectory() as tmp:
        shutil.copytree("tests", pathlib.Path(tmp) / "tests")
        (pathlib.Path(tmp) / "wrap.py").write_text(wrap_source)
        run = subprocess.run([sys.executable, "-m", "unittest", "-q", "tests.test_wrap"],
                             cwd=tmp, capture_output=True, text=True)
        return run.returncode == 0


assert pathlib.Path("tests/test_wrap.py").is_file(), "no tests/test_wrap.py"
assert tests_pass(pathlib.Path("wrap.py").read_text()), "the tests fail on the real wrap.py"
missed = [m.stem for m in sorted((HERE / "mutants").glob("*.py")) if tests_pass(m.read_text())]
assert not missed, f"the tests pass on broken versions: {missed}"
