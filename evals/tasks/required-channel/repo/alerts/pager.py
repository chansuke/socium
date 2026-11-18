import functools

import notify

page = functools.partial(notify.send, retries=5)


def wake(who):
    return page(f"wake {who}")
