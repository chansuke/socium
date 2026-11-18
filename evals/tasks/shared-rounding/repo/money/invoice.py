from money.convert import format_cents, to_cents


def total(lines):
    """lines: (unit price in dollars, quantity) pairs."""
    return sum(to_cents(price) * qty for price, qty in lines)


def render(lines):
    return f"Total: {format_cents(total(lines))}"
