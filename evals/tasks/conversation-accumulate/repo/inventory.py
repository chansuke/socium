def restock(stock, target):
    """Brings every item below `target` up to `target`. Returns the new stock."""
    return {name: max(qty, target) for name, qty in stock.items()}
