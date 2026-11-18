import re

PATTERN = re.compile(r"(?:(\d+)h)?(?:(\d+)m)?")


def parse_duration(text):
    """Seconds in a duration such as 2h, 45m, or 1h30m."""
    match = PATTERN.fullmatch(text.strip())
    if not match or match.groups() == (None, None):
        raise ValueError(f"not a duration: {text!r}")
    hours, minutes = match.groups()
    return int(hours or 0) * 3600 + int(minutes or 0) * 60
