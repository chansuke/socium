from money.convert import to_cents


def refund(paid_cents, returned_amount):
    """Cents still kept after returning `returned_amount` dollars."""
    return paid_cents - to_cents(returned_amount)
