from netkit.conn import connect


def open_local(port=8080):
    return connect(f"[::1]:{port}")
