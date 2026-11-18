def subtotal(cart):
    return sum(cents * qty for _, cents, qty in cart.lines)


GIFT_WRAP_CENTS = 300


def total(cart):
    return subtotal(cart) + (GIFT_WRAP_CENTS if getattr(cart, "gift_wrap", False) else 0)
