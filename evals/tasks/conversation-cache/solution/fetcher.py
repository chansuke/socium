import time


class Fetcher:
    """Reads URLs with `get`, a function from URL to body, keeping each body
    for `ttl` seconds by `clock`."""

    def __init__(self, get, ttl=60, clock=time.monotonic):
        self._get = get
        self._ttl = ttl
        self._clock = clock
        self._cache = {}

    def fetch(self, url):
        now = self._clock()
        entry = self._cache.get(url)
        if entry is not None and now - entry[0] <= self._ttl:
            return entry[1]
        body = self._get(url)
        self._cache[url] = (now, body)
        return body

    def clear(self):
        self._cache.clear()
