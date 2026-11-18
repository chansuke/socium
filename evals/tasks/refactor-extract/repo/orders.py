def create_order(qty, price):
    if not isinstance(qty, int) or qty <= 0:
        raise ValueError("qty must be a positive integer")
    if price < 0:
        raise ValueError("price must not be negative")
    return {"qty": qty, "price": price, "total": qty * price}


def update_order(order, qty):
    if not isinstance(qty, int) or qty <= 0:
        raise ValueError("qty must be a positive integer")
    return {**order, "qty": qty, "total": qty * order["price"]}
