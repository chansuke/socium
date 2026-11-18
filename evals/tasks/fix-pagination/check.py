import subprocess
import sys

sys.path.insert(0, ".")
from pager import page, page_count

items = list(range(1, 24))
assert page(items, 1, 10) == list(range(1, 11))
assert page(items, 3, 10) == [21, 22, 23]
assert page(items, 4, 10) == []
assert page([], 1, 5) == []
assert page_count(items, 10) == 3
assert page_count([], 10) == 0
assert page_count(list(range(20)), 10) == 2
try:
    page(items, 0, 10)
except ValueError:
    pass
else:
    raise AssertionError("page 0 must raise ValueError")
sys.exit(subprocess.run([sys.executable, "-m", "unittest", "-q"]).returncode)
