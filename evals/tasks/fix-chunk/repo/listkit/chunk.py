def chunk(items, size):
    """Splits items into lists of `size`; the last may be shorter."""
    if size < 1:
        raise ValueError("size must be at least 1")
    items = list(items)
    return [items[i : i + size] for i in range(0, len(items) - size + 1, size)]
