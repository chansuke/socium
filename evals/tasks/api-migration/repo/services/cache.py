from netkit.conn import connect

PRIMARY = ("cache-1.internal", 6379)


def open_cache():
    return connect(*PRIMARY)
