#!/usr/bin/env python3
"""Minimal RCON client for the local test server: rcon.py <command>..."""
import os, socket, struct, sys
def pkt(i, t, body):
    b = body.encode() + b"\0\0"
    return struct.pack("<iii", len(b) + 8, i, t) + b
def recv(s):
    n = struct.unpack("<i", s.recv(4))[0]
    data = b""
    while len(data) < n:
        data += s.recv(n - len(data))
    return data[8:-2].decode(errors="replace")
# RCON_PORT picks the server when several run side by side (one per agent).
s = socket.create_connection(("127.0.0.1", int(os.environ.get("RCON_PORT", "25575"))))
s.sendall(pkt(1, 3, "testpass")); recv(s)
for cmd in sys.argv[1:]:
    s.sendall(pkt(2, 2, cmd)); print(recv(s))
