import random

BASE = 0.5
CAP = 30.0


def next_delay(attempt):
    """Seconds to wait before attempt number `attempt + 1`."""
    return min(CAP, BASE * 2**attempt) * random.uniform(0.5, 1.0)
