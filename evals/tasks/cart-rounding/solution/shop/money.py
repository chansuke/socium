from decimal import ROUND_HALF_UP, Decimal


def to_cents(amount):
    """A dollar amount, as a string or a number, in whole cents, rounding half up."""
    cents = Decimal(str(amount)) * 100
    return int(cents.quantize(Decimal(1), rounding=ROUND_HALF_UP))


def format_cents(cents):
    return f"${cents // 100}.{cents % 100:02d}"
