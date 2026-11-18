def _send(method, url, timeout):
    return (method, url, timeout)


def get(url):
    return _send("GET", url, timeout=30)


def post(url, body):
    return _send("POST", url + "?len=" + str(len(body)), timeout=30)


def remove(url):
    return _send("DELETE", url, timeout=30)


def purge_before(today):
    """The first day still kept: logs are kept for 30 days."""
    return today - 30
