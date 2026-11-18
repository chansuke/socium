def cents_to_units(cents):
    """Whole units and remaining cents."""
    return divmod(cents, 100)
