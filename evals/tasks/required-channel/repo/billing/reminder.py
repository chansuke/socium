from notify import send as deliver


def remind(user):
    return deliver("reminder for " + user)
