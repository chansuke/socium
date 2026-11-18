import re
import unicodedata


def slugify_title(title):
    """Turns a post title into a lowercase ASCII URL fragment."""
    ascii_text = unicodedata.normalize("NFKD", title).encode("ascii", "ignore").decode()
    words = re.split(r"[^a-z0-9]+", ascii_text.lower())
    return "-".join(word for word in words if word)
