from netkit.conn import connect


def open_metrics():
    return connect("metrics.internal:8125")
