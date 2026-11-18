from shop.money import format_cents, to_cents
from shop.pricing import discounted


def total(lines):
    """The total of (price, quantity, percent off) lines."""
    cents = sum(discounted(to_cents(price), percent) * qty for price, qty, percent in lines)
    return format_cents(cents)
