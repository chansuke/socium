def to_cents(amount):
    """Dollars to whole cents."""
    return int(round(float(amount) * 100))


def format_cents(cents):
    sign = "-" if cents < 0 else ""
    return f"{sign}${abs(cents) // 100}.{abs(cents) % 100:02d}"
