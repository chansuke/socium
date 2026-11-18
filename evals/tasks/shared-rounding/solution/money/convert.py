from decimal import ROUND_HALF_UP, Decimal


def to_cents(amount):
    """Dollars to whole cents, half away from zero."""
    return int((Decimal(str(amount)) * 100).quantize(Decimal("1"), rounding=ROUND_HALF_UP))


def format_cents(cents):
    sign = "-" if cents < 0 else ""
    return f"{sign}${abs(cents) // 100}.{abs(cents) % 100:02d}"
