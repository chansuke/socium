def subtotal(cart):
    return sum(cents * qty for _, cents, qty in cart.lines)


def total(cart):
    return subtotal(cart)
