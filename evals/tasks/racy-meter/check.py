import sys
import time

sys.path.insert(0, ".")
from meter import Meter, run_workers

for _ in range(3):
    seen = []

    def hook(total):
        time.sleep(0)
        seen.append(total)

    meter = Meter(hook)
    assert run_workers(meter, 8, 500) == 4000, meter.total
    assert seen == list(range(1, 4001)), "on_change totals are missing or out of order"
assert Meter().add(2) is None
