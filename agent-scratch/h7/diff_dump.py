#!/usr/bin/env python3
"""Byte-compare two h7bench --dump op streams. Exit 0 = identical (parity), 1 = diff."""
import sys

a = open(sys.argv[1], "rb").read()
b = open(sys.argv[2], "rb").read()
if a == b:
    print(f"IDENTICAL ({len(a)} bytes) — op-stream parity HOLDS")
    sys.exit(0)
n = min(len(a), len(b))
first = next((i for i in range(n) if a[i] != b[i]), n)
print(f"DIFFER: len {len(a)} vs {len(b)}; first byte diff at {first}")
# framed format: u32 name_len, name, u32 nops, ops*6*i32 — print frame context
def frames(buf):
    off, out = 8, []  # skip magic
    while off + 8 <= len(buf):
        nl = int.from_bytes(buf[off:off+4], "little"); off += 4
        name = buf[off:off+nl].decode(errors="replace"); off += nl
        nops = int.from_bytes(buf[off:off+4], "little"); off += 4
        out.append((name, nops, off))
        off += nops * 24
    return out
fa, fb = frames(a), frames(b)
print(f"frames: {len(fa)} vs {len(fb)}")
diffs = 0
for i, ((na, ka, oa), (nb, kb, ob)) in enumerate(zip(fa, fb)):
    if na != nb or ka != kb:
        diffs += 1
        if diffs <= 8:
            print(f"  frame {i}: {na}/{ka} vs {nb}/{kb}")
print("frames differing:", diffs)
sys.exit(1)
