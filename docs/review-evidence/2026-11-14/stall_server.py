"""Accepts connections on a loopback port and never answers."""
import socket, time
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 18765))
s.listen(8)
held = []
while True:
    conn, _ = s.accept()
    held.append(conn)
