import notify

SENDERS = [notify.send]


def broadcast(message):
    return [sender(message, channel="sms") for sender in SENDERS]
