import sys

sys.path.insert(0, ".")
from fetcher import Fetcher

calls = []
fail = set()


def get(url):
    calls.append(url)
    if url in fail:
        raise IOError(url)
    return f"body {url} #{len(calls)}"


now = [100.0]
f = Fetcher(get, ttl=10, clock=lambda: now[0])
assert f.fetch("a") == "body a #1"
assert f.fetch("a") == "body a #1"
assert f.fetch("b") == "body b #2"
now[0] = 110.0
assert f.fetch("a") == "body a #1", "an entry exactly ttl old must be used"
now[0] = 110.5
assert f.fetch("a") == "body a #3", "an entry older than ttl must be read again"
fail.add("c")
for _ in range(2):
    try:
        f.fetch("c")
    except IOError:
        pass
    else:
        raise AssertionError("the error must pass through")
assert calls.count("c") == 2, "a failure was cached"
fail.clear()
assert f.fetch("c") == f"body c #{len(calls)}"
assert f.fetch("c") == f"body c #{len(calls)}"
f.clear()
before = len(calls)
f.fetch("b")
assert len(calls) == before + 1, "clear() kept entries"
plain = Fetcher(get)
plain.fetch("z")
plain.fetch("z")
assert calls.count("z") == 1, "the defaults must cache"
