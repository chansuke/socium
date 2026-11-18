from netkit.conn import connect as dial


def open_queue():
    return dial("mq.internal", port=5672)
