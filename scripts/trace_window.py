#!/usr/bin/env python3
"""Filter DecoDriver/native trace for one chunk region, in file order.
Usage: trace_window.py <run.out> <x0> <x1> <z0> <z1>"""
import re, sys

pat = re.compile(
    r"(?P<kind>\[leaf\]|\[biomef\]|\[rsel@|\[srs@|\[rbs@|\[op@|\[jni-dbg\] height|\[swd\]|\[draw\])"
)
leaf = re.compile(r"\[leaf\] uid=(\S+) (\d+) (\d+) (\d+)")
biomef = re.compile(r"\[biomef\] (\d+) (\d+) (\d+) uid=(\S+) -> (\S+)")
jni = re.compile(r"\[jni-dbg\] height ty=(\d+) x=(\d+) z=(\d+) -> (\d+)")
op = re.compile(r"\[op@(\d+)\] w \((\d+),(-?\d+),(\d+)\) (\S+) flags=(\S+)")

def main():
    path = sys.argv[1]
    x0, x1, z0, z1 = map(int, sys.argv[2:6])
    with open(path, errors="replace") as f:
        for ln, line in enumerate(f, 1):
            if not pat.search(line):
                continue
            keep = False
            m = leaf.search(line)
            if m:
                x, y, z = int(m.group(2)), int(m.group(3)), int(m.group(4))
                keep = x0 <= x <= x1 and z0 <= z <= z1
                if keep:
                    print(f"{ln}: LEAF uid={m.group(1)} ({x},{y},{z})")
                continue
            m = biomef.search(line)
            if m:
                x, z = int(m.group(1)), int(m.group(3))
                keep = x0 <= x <= x1 and z0 <= z <= z1
                if keep:
                    print(f"{ln}: BIOMEF ({x},{m.group(2)},{z}) uid={m.group(4)} -> {m.group(5)}")
                continue
            m = jni.search(line)
            if m:
                x, z = int(m.group(2)), int(m.group(3))
                keep = x0 <= x <= x1 and z0 <= z <= z1
                if keep:
                    print(f"{ln}: HEIGHT ty={m.group(1)} ({x},{z}) -> {m.group(4)}")
                continue
            m = op.search(line)
            if m:
                x, y, z = int(m.group(2)), int(m.group(3)), int(m.group(4))
                keep = x0 <= x <= x1 and z0 <= z <= z1
                if keep:
                    print(f"{ln}: OP ({x},{y},{z}) {m.group(5)} flags={m.group(6)}")
                continue
            print(f"{ln}: {line.rstrip()}")

if __name__ == "__main__":
    main()
