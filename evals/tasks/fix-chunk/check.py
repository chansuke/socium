import sys

sys.path.insert(0, ".")
from listkit.chunk import chunk

assert chunk([1, 2, 3, 4, 5], 2) == [[1, 2], [3, 4], [5]]
assert chunk([1, 2, 3, 4], 2) == [[1, 2], [3, 4]]
assert chunk([1, 2], 5) == [[1, 2]]
assert chunk([], 3) == []
assert chunk(range(3), 1) == [[0], [1], [2]]
for size in (0, -1):
    try:
        chunk([1], size)
    except ValueError:
        pass
    else:
        raise AssertionError("size below 1 must raise ValueError")
