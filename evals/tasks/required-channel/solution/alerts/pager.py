import functools

import notify

page = functools.partial(notify.send, retries=5, channel="sms")


def wake(who):
    return page(f"wake {who}")
