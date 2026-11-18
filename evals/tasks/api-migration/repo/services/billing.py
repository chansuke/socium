from netkit.conn import connect


def open_db():
    return connect("db.internal", 5432)
