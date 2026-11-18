import pathlib
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).parent
sys.path.insert(0, ".")
from stats import median

assert median([3, 1, 2]) == 2
assert median([4, 1, 2, 3]) == 2.5
try:
    median([])
except ValueError as err:
    assert str(err) == "median of empty data", err
else:
    raise AssertionError("median([]) must raise ValueError")


def tests_pass(stats_source):
    with tempfile.TemporaryDirectory() as tmp:
        shutil.copytree("tests", pathlib.Path(tmp) / "tests")
        (pathlib.Path(tmp) / "stats.py").write_text(stats_source)
        return subprocess.run([sys.executable, "-m", "unittest", "-q"], cwd=tmp,
                              capture_output=True).returncode == 0


assert pathlib.Path("tests/test_stats.py").is_file(), "no tests/test_stats.py"
assert tests_pass(pathlib.Path("stats.py").read_text()), "the tests fail on the fix"
assert not tests_pass((HERE / "repo" / "stats.py").read_text()), "the tests pass without the fix"
