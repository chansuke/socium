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


def compare(a, b):
    """-1, 0, or 1 as version a is lower than, equal to, or higher than b."""
    raise NotImplementedError


def latest(versions):
    """The highest of `versions`."""
    raise NotImplementedError
