REQUEST_TIMEOUT = 30


def _send(method, url, timeout):
    return (method, url, timeout)


def get(url):
    return _send("GET", url, timeout=REQUEST_TIMEOUT)


def post(url, body):
    return _send("POST", url + "?len=" + str(len(body)), timeout=REQUEST_TIMEOUT)


def remove(url):
    return _send("DELETE", url, timeout=REQUEST_TIMEOUT)


def purge_before(today):
    """The first day still kept: logs are kept for 30 days."""
    return today - 30
