import notify


def issue(number):
    return notify.send(f"invoice {number}", retries=3, channel="email")
