#!/usr/bin/env python3
"""Minimal stdlib-only RCON client (Source RCON protocol).

Usage:
    python3 rcon.py <host> <port> <password> "command"

Prints the server response body to stdout.
Exit codes: 0 = command ok, 2 = auth failure, 3 = protocol/connection error.
"""
import socket
import struct
import sys

SERVERDATA_AUTH = 3
SERVERDATA_AUTH_RESPONSE = 2
SERVERDATA_EXECCOMMAND = 2
SERVERDATA_RESPONSE_VALUE = 0


def recv_exact(sock: socket.socket, n: int) -> bytes:
    buf = b""
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise ConnectionError("connection closed by server")
        buf += chunk
    return buf


def send_packet(sock: socket.socket, req_id: int, ptype: int, payload: str) -> None:
    body = struct.pack("<ii", req_id, ptype) + payload.encode("utf-8") + b"\x00\x00"
    sock.sendall(struct.pack("<i", len(body)) + body)


def read_packet(sock: socket.socket):
    (length,) = struct.unpack("<i", recv_exact(sock, 4))
    data = recv_exact(sock, length)
    rid, rtype = struct.unpack("<ii", data[:8])
    body = data[8:-2].decode("utf-8", "replace")
    return rid, rtype, body


def connect(host: str, port: int, password: str, timeout: float = 600.0) -> socket.socket:
    sock = socket.create_connection((host, port), timeout=10.0)
    sock.settimeout(timeout)
    send_packet(sock, 1, SERVERDATA_AUTH, password)
    # Server may send an empty RESPONSE_VALUE before AUTH_RESPONSE; loop until auth response.
    while True:
        rid, rtype, _ = read_packet(sock)
        if rtype == SERVERDATA_AUTH_RESPONSE:
            if rid == -1:
                raise PermissionError("RCON auth failed (bad password)")
            break
    return sock


def command(sock: socket.socket, cmd: str, req_id: int = 2) -> str:
    send_packet(sock, req_id, SERVERDATA_EXECCOMMAND, cmd)
    rid, rtype, body = read_packet(sock)
    if rtype != SERVERDATA_RESPONSE_VALUE:
        raise ProtocolError(f"unexpected response type {rtype}")
    return body


class ProtocolError(RuntimeError):
    pass


def main(argv) -> int:
    if len(argv) < 5:
        sys.stderr.write(__doc__)
        return 3
    host, port_s, password, cmd = argv[1], argv[2], argv[3], argv[4]
    try:
        port = int(port_s)
    except ValueError:
        sys.stderr.write(f"bad port: {port_s}\n")
        return 3
    try:
        sock = connect(host, port, password)
    except PermissionError as e:
        sys.stderr.write(f"AUTH_FAIL: {e}\n")
        return 2
    except (OSError, ConnectionError) as e:
        sys.stderr.write(f"CONN_FAIL: {e}\n")
        return 3
    try:
        print(command(sock, cmd))
        return 0
    except (OSError, ConnectionError, ProtocolError) as e:
        sys.stderr.write(f"CMD_FAIL: {e}\n")
        return 3
    finally:
        sock.close()


if __name__ == "__main__":
    sys.exit(main(sys.argv))
