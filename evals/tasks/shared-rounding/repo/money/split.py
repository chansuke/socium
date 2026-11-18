from money.convert import to_cents


def per_person(amount, people):
    """Each share of `amount` dollars, in cents; the first shares take the remainder."""
    cents = to_cents(amount)
    base, extra = divmod(cents, people)
    return [base + (1 if i < extra else 0) for i in range(people)]
