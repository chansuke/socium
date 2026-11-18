import sys

sys.path.insert(0, ".")
from semver import compare, latest

order = ["0.9.0", "1.0.0-0", "1.0.0-2", "1.0.0-10", "1.0.0-B", "1.0.0-a", "1.0.0-alpha",
         "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11",
         "1.0.0-rc.1", "1.0.0", "1.0.1", "1.9.0", "1.10.0", "2.0.0-x-y", "2.0.0"]
for i, a in enumerate(order):
    for j, b in enumerate(order):
        expected = (i > j) - (i < j)
        assert compare(a, b) == expected, (a, b, compare(a, b), expected)
assert compare("1.0.0+a", "1.0.0+b") == 0
assert compare("1.0.0-rc.1+x", "1.0.0-rc.1") == 0
assert latest(["1.0.0+b", "1.0.0-rc.1", "1.0.0+a"]) == "1.0.0+b"
assert latest(["1.10.0", "1.9.9"]) == "1.10.0"
assert latest(iter(["0.1.0"])) == "0.1.0"
try:
    latest([])
except ValueError:
    pass
else:
    raise AssertionError("latest([]) must raise ValueError")
