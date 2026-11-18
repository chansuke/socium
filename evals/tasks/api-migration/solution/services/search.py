from netkit import conn


def open_index():
    return conn.connect("search.internal:9200")
