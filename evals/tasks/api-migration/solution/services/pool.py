from netkit.conn import connect

HOSTS = ["w1.internal", "w2.internal"]


def open_pool():
    return [connect(f"{h}:{7000 + i}") for i, h in enumerate(HOSTS)]
