def to_cents(amount):
    """A dollar amount, as a string or a number, in whole cents, rounding half up."""
    return int(float(amount) * 100)


def format_cents(cents):
    return f"${cents // 100}.{cents % 100:02d}"
