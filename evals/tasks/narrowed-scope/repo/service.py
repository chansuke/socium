def quote(amount):
    return round(amount * 1.19, 2)


def charge(account, amount):
    account["balance"] -= amount
    return account["balance"]


def refund(account, amount):
    account["balance"] += amount
    return account["balance"]
