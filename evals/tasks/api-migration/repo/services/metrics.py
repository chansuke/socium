import functools

from netkit.conn import connect

to_metrics = functools.partial(connect, "metrics.internal")


def open_metrics():
    return to_metrics(8125)
