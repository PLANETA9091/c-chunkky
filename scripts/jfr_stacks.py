#!/usr/bin/env python3
"""jfr_stacks.py <jfr_print.txt> <regex> [max] — print full stacks of matching events."""
import re, sys

txt = open(sys.argv[1], errors="replace").read()
pat = re.compile(sys.argv[2])
maxn = int(sys.argv[3]) if len(sys.argv) > 3 else 5

EV = re.compile(r"^jdk\.(ExecutionSample|NativeMethodSample) \{$")
THREAD = re.compile(r'sampledThread = "([^"]*)"')
FRAME = re.compile(r"^        (.+?) line: \d+$|^        (.+?)$")

events = []
cur = None
for line in txt.splitlines():
    if EV.match(line):
        cur = {"ev": line.split()[0], "thread": "?", "frames": []}
        events.append(cur)
        continue
    if cur is None:
        continue
    m = THREAD.search(line)
    if m and "sampledThread" in line:
        cur["thread"] = m.group(1)
        continue
    if line == "}":
        cur = None
        continue
    if cur["frames"] is not None and line.startswith("    ") and line[4:5] not in ("", " "):
        f = line.strip()
        if f == "...":
            cur["trunc"] = True
            continue
        cur["frames"].append(f)

shown = 0
agg = {}
for e in events:
    joined = "\n".join(e["frames"])
    if pat.search(joined):
        key = "\n".join(e["frames"][:14])
        agg[key] = agg.get(key, 0) + 1

for k, v in sorted(agg.items(), key=lambda x: -x[1])[:maxn]:
    print(f"=== x{v}")
    print(k)
    print()
