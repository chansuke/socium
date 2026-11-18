import warnings


class Connection:
    def __init__(self, host, port):
        self.host = host
        self.port = port

    def __repr__(self):
        return f"Connection({self.host!r}, {self.port})"


def _split(address):
    if address.startswith("["):
        host, _, port = address[1:].partition("]:")
    else:
        host, _, port = address.rpartition(":")
    if not host or not port.isdigit():
        raise ValueError(f"not host:port: {address!r}")
    return host, int(port)


def connect(address=None, port=None, *, host=None):
    """connect("host:port"), or "[v6]:port". connect(host, port) still works
    for now but is deprecated."""
    if host is not None or port is not None:
        warnings.warn("connect(host, port) is deprecated; pass 'host:port'", DeprecationWarning, stacklevel=2)
        return Connection(address if host is None else host, int(port))
    return Connection(*_split(address))
