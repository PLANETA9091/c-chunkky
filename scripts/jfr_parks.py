#!/usr/bin/env python3
"""jfr_parks.py <jfr_print.txt> — ThreadPark analysis of Paper Common Worker threads.

Reads `jfr print` text output (jdk.ThreadPark + jdk.ExecutionSample events) and
reports, per worker thread and total:
  - window span (first/last event timestamp)
  - park count + total parked time; share of the window span
  - park-site classification (top stack leaf, deduped)
  - execution samples per thread (RUNNABLE proxy) -> busy share
Prints a compact verdict for the dispatch/idle residue question.
"""
import re
import sys

WORKER = re.compile(r"Paper Common Worker #(\d+)")
TS = re.compile(r"startTime = (\d+):(\d+):(\d+)\.(\d+)")
DUR = re.compile(r"duration = ([\d.]+) (ms|s|us|ns)")
STATE = re.compile(r'state = "(STATE_[A-Z]+)"')
LEAF = re.compile(r"^    (.+?)(?: line: \d+)?$")

events = []  # (t_ms, kind, thread, extra)
cur = None   # dict of current event


def t2ms(h, m, s, frac):
    return ((int(h) * 60 + int(m)) * 60 + int(s)) * 1000 + int(frac) / 10.0


def dur2ms(val, unit):
    v = float(val)
    return {"ms": v, "s": v * 1000, "us": v / 1000, "ns": v / 1e6}[unit]


for raw in open(sys.argv[1], errors="replace"):
    line = raw.rstrip("\n")
    if line.startswith("jdk.ThreadPark {") or line.startswith("jdk.ExecutionSample {"):
        cur = {"kind": line.split()[0], "thread": "?", "stack": [], "leaf": "?"}
        continue
    if cur is None:
        continue
    if line == "}":
        if cur["stack"]:
            m = LEAF.match(cur["stack"][0])
            if m:
                cur["leaf"] = m.group(1).strip()
        events.append(cur)
        cur = None
        continue
    mt = WORKER.search(line)
    if mt and ("sampledThread" in line or "eventThread" in line):
        cur["thread"] = mt.group(0)
        continue
    ms = TS.search(line)
    if ms:
        cur["t"] = t2ms(*ms.groups())
        continue
    md = DUR.search(line)
    if md and cur["kind"] == "jdk.ThreadPark":
        cur["dur"] = dur2ms(*md.groups())
        continue
    if line.startswith("    "):
        cur["stack"].append(line)

if not events:
    print("no events parsed")
    sys.exit(0)

t0 = min(e["t"] for e in events if "t" in e)
t1 = max(e["t"] for e in events if "t" in e)
span = t1 - t0
print(f"window span ~{span/1000:.2f}s ({t1-t0:.0f} ms), events={len(events)}")

parks = [e for e in events if e["kind"] == "jdk.ThreadPark" and "Paper Common Worker" in e["thread"]]
execs = [e for e in events if e["kind"] == "jdk.ExecutionSample" and "Paper Common Worker" in e["thread"]]

per = {}
for e in parks:
    per.setdefault(e["thread"], []).append(e)
for w in sorted(per, key=lambda x: int(WORKER.search(x).group(1))):
    es = per[w]
    tot = sum(x.get("dur", 0) for x in es)
    print(f"\n{w}: parks={len(es)} parked_total={tot/1000:.2f}s share={100*tot/span:.1f}% max={max(x.get('dur',0) for x in es):.0f}ms")
    leaves = {}
    for x in es:
        leaves[x["leaf"]] = leaves.get(x["leaf"], 0) + 1
    for leaf, n in sorted(leaves.items(), key=lambda kv: -kv[1])[:4]:
        print(f"    {n:4d}  {leaf[:110]}")

tot_park = sum(x.get("dur", 0) for x in parks)
print(f"\nALL workers: parks={len(parks)} parked_total={tot_park/1000:.2f}s share_of_window={100*tot_park/span:.1f}%")

perw = {}
for e in execs:
    perw[e["thread"]] = perw.get(e["thread"], 0) + 1
print("exec samples per worker (RUNNABLE proxy):")
for w in sorted(perw, key=lambda x: int(WORKER.search(x).group(1))):
    print(f"    {w}: {perw[w]}")
print(f"total exec samples on workers: {len(execs)}")

long_parks = [x for x in parks if x.get("dur", 0) > 50]
print(f"parks >50ms: {len(long_parks)}")
for x in sorted(long_parks, key=lambda y: -y.get("dur", 0))[:6]:
    print(f"    {x.get('dur',0):.0f}ms {x['thread']} <- {x['leaf'][:90]}")
