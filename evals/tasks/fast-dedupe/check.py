import pathlib
import random
import subprocess
import sys

HERE = pathlib.Path(__file__).parent
sys.path.insert(0, ".")
from events import latest_versions

reference = {}
exec((HERE / "reference.py").read_text(), reference)
rng = random.Random(7)
for _ in range(300):
    n = rng.randint(0, 40)
    events = [{"id": rng.randint(0, 8), "version": rng.randint(0, 4), "n": k} for k in range(n)]
    got = latest_versions([dict(e) for e in events])
    want = reference["latest_versions"]([dict(e) for e in events])
    assert got == want, (events, got, want)
odd = [{"id": (1, "a"), "version": 1.5, "n": 0}, {"id": (1, "a"), "version": 1.5, "n": 1}, {"id": None, "version": -1, "n": 2}]
assert latest_versions(odd) == reference["latest_versions"](odd)

timed = """
import random, sys, time
sys.path.insert(0, ".")
from events import latest_versions
rng = random.Random(1)
events = [{"id": rng.randrange(100_000), "version": rng.randrange(50)} for _ in range(200_000)]
start = time.perf_counter()
latest_versions(events)
print(time.perf_counter() - start)
"""
run = subprocess.run([sys.executable, "-c", timed], capture_output=True, text=True, timeout=30)
assert run.returncode == 0, run.stderr
assert float(run.stdout) < 3.0, f"took {run.stdout.strip()}s"
