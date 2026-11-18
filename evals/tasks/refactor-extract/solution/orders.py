def _check_qty(qty):
    if not isinstance(qty, int) or qty <= 0:
        raise ValueError("qty must be a positive integer")


def create_order(qty, price):
    _check_qty(qty)
    if price < 0:
        raise ValueError("price must not be negative")
    return {"qty": qty, "price": price, "total": qty * price}


def update_order(order, qty):
    _check_qty(qty)
    return {**order, "qty": qty, "total": qty * order["price"]}
