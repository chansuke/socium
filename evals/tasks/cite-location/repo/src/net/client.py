from .backoff import next_delay


def fetch(session, url, attempts=5):
    for attempt in range(attempts):
        response = session.get(url)
        if response.ok:
            return response
        session.sleep(next_delay(attempt))
    raise RuntimeError(f"gave up on {url}")
