def squeeze(text):
    """Trims the ends and turns runs of whitespace into one space."""
    return " ".join(text.split())


def fold(text):
    """Case-folds for comparison."""
    return text.casefold()
