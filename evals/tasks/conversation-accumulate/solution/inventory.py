def restock(stock, target, *, skip=frozenset()):
    """Brings every item below `target` up to `target`, except those in
    `skip`. Returns the new stock and the number of units added."""
    new = {name: qty if name in skip else max(qty, target) for name, qty in stock.items()}
    return new, sum(new.values()) - sum(stock.values())
