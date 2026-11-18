import re

PATTERN = re.compile(
    r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)


def parse(text):
    """(major, minor, patch, pre-release identifiers, build identifiers)."""
    match = PATTERN.fullmatch(text)
    if not match:
        raise ValueError(f"not a version: {text!r}")
    major, minor, patch, pre, build = match.groups()
    return (
        int(major),
        int(minor),
        int(patch),
        tuple(pre.split(".")) if pre else (),
        tuple(build.split(".")) if build else (),
    )


def _compare_pre(a, b):
    for x, y in zip(a, b):
        if x == y:
            continue
        if x.isdigit() and y.isdigit():
            return -1 if int(x) < int(y) else 1
        if x.isdigit() != y.isdigit():
            return -1 if x.isdigit() else 1
        return -1 if x < y else 1
    return (len(a) > len(b)) - (len(a) < len(b))


def compare(a, b):
    """-1, 0, or 1 as version a is lower than, equal to, or higher than b."""
    a, b = parse(a), parse(b)
    if a[:3] != b[:3]:
        return -1 if a[:3] < b[:3] else 1
    if not a[3] or not b[3]:
        return (not a[3]) - (not b[3])
    return _compare_pre(a[3], b[3])


def latest(versions):
    """The highest of `versions`."""
    best = None
    for version in versions:
        if best is None or compare(version, best) > 0:
            best = version
    if best is None:
        raise ValueError("no versions")
    return best
