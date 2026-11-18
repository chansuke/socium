import re


def slugify(title):
    """Join the words of a title with hyphens, in lowercase."""
    words = re.findall(r"[a-z0-9]+", title)
    return "-".join(words)
