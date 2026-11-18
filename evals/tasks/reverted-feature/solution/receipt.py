from pricing import subtotal, total


def money(cents):
    return f"${cents // 100}.{cents % 100:02d}"


def receipt(cart):
    lines = [f"{name} x{qty}: {money(cents * qty)}" for name, cents, qty in cart.lines]
    lines.append(f"Subtotal: {money(subtotal(cart))}")
    if getattr(cart, "gift_wrap", False):
        lines.append("Gift wrap: $3.00")
    lines.append(f"Total: {money(total(cart))}")
    return "\n".join(lines)
