import notify

SENDERS = [notify.send]


def broadcast(message):
    return [sender(message) for sender in SENDERS]
