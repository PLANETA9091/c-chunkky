#!/usr/bin/env python3
"""winpoll.py — poll the honest-window in-count over ONE rcon connection.

Usage: winpoll.py <host> <port> <password> <target> [timeout_s]
Polls `wincount` every 0.5s until window_inside >= target, then prints the
final count. Exits 0 on success, 1 on timeout (count printed anyway).
"""
import socket
import sys
import time

sys.path.insert(0, "/home/z/my-project/c-faster/harness")
import rcon  # noqa: E402


def main() -> int:
    host, port, password, target = sys.argv[1], int(sys.argv[2]), sys.argv[3], int(sys.argv[4])
    timeout = float(sys.argv[5]) if len(sys.argv) > 5 else 120.0
    sock = rcon.connect(host, port, password)
    t0 = time.time()
    last = -1
    try:
        while time.time() - t0 < timeout:
            out = rcon.command(sock, "wincount")
            inside = 0
            for tok in out.split():
                if tok.startswith("window_inside="):
                    inside = int(tok.split("=", 1)[1])
            if inside != last:
                last = inside
            if inside >= target:
                print(f"WIN_DONE {inside}")
                return 0
            time.sleep(0.5)
        print(f"WIN_TIMEOUT {last}")
        return 1
    finally:
        sock.close()


if __name__ == "__main__":
    sys.exit(main())
