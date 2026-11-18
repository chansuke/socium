_transport = print


def send(message, retries=1):
    """Sends `message`, trying up to `retries` times."""
    for _ in range(retries):
        if _transport(message) is not False:
            return True
    return False
