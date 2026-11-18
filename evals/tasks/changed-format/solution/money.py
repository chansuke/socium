def cents_to_units(cents):
    """Whole units and remaining cents."""
    return divmod(cents, 100)


def format_price(cents):
    """Euros in German style, such as 1.234,50 €."""
    units, rest = cents_to_units(cents)
    return f"{units:,}".replace(",", ".") + f",{rest:02d} €"
