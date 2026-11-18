class Fetcher:
    """Reads URLs with `get`, a function from URL to body."""

    def __init__(self, get):
        self._get = get

    def fetch(self, url):
        return self._get(url)
