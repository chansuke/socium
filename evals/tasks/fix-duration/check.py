import sys

sys.path.insert(0, ".")
from durations import parse_duration

cases = {"45m": 2700, "2h": 7200, "1h30m": 5400, "0m": 0, "10h5m": 36300, " 3m ": 180}
for text, seconds in cases.items():
    assert parse_duration(text) == seconds, (text, parse_duration(text))
for bad in ("", "abc", "5", "m", "1x"):
    try:
        parse_duration(bad)
    except ValueError:
        pass
    else:
        raise AssertionError(f"{bad!r} must raise ValueError")
