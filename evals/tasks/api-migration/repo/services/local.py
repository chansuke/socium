from netkit.conn import connect


def open_local(port=8080):
    return connect("::1", port)
