import importlib.util
import pathlib
import random
import signal
import sys
import time

sys.path.insert(0, ".")
import events

spec = importlib.util.spec_from_file_location("reference", pathlib.Path(__file__).parent / "reference.py")
reference = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference)

rng = random.Random(7)
for _ in range(300):
    evs = [{"id": rng.choice([rng.randrange(20), str(rng.randrange(5))]), "n": i} for i in range(rng.randrange(40))]
    assert events.dedupe(evs) == reference.dedupe(evs)
    spans = []
    for _ in range(rng.randrange(30)):
        a = rng.randrange(-20, 60)
        spans.append((a, a + rng.randrange(-1, 8)))
    spans = [(a, b) for a, b in spans if a <= b]
    assert events.merge_ranges(spans) == reference.merge_ranges(spans), spans

signal.alarm(30)
big = [{"id": i % 150_000, "n": i} for i in range(300_000)]
started = time.monotonic()
assert len(events.dedupe(big)) == 150_000
disjoint = [(i * 3, i * 3 + 1) for i in range(200_000)]
rng.shuffle(disjoint)
assert len(events.merge_ranges(disjoint)) == 200_000
elapsed = time.monotonic() - started
assert elapsed < 5, f"took {elapsed:.1f}s"
