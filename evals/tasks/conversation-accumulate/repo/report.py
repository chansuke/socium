from inventory import restock


def weekly(stock):
    new = restock(stock, 10)
    return ", ".join(f"{name}={qty}" for name, qty in sorted(new.items()))
