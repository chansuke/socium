from netkit import conn


def open_index():
    return conn.connect(host="search.internal", port=9200)
