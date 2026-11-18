class Connection:
    def __init__(self, host, port):
        self.host = host
        self.port = port

    def __repr__(self):
        return f"Connection({self.host!r}, {self.port})"


def connect(host, port):
    return Connection(host, port)
