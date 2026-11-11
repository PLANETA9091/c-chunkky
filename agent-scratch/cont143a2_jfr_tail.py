#!/usr/bin/env python3
"""Dump HEAD (leaf) + TAIL (top 12 frames) of window stacks for given leaf keys."""
import re, collections

PATH = "/home/z/my-project/c-faster/reference/jfr_cont143/jfr_print.txt"
WIN = "17:11:00"
rec_re = re.compile(r'startTime = (\d\d:\d\d:\d\d\.\d+)')
thr_re = re.compile(r'sampledThread = "([^"]+)"')

samples = []
with open(PATH) as f:
    cur_t = cur_thr = None; in_stack = False; stack = []
    for line in f:
        if line.startswith("jdk.ExecutionSample"):
            cur_t = cur_thr = None; stack = []; in_stack = False; continue
        m = rec_re.search(line)
        if m and cur_t is None: cur_t = m.group(1); continue
        m = thr_re.search(line)
        if m and cur_thr is None: cur_thr = m.group(1); continue
        if line.startswith("  stackTrace = ["): in_stack = True; continue
        if in_stack:
            if line.startswith("  ]"):
                in_stack = False
                if cur_t and cur_t >= WIN: samples.append((cur_thr, stack))
                stack = []
            else:
                fr = line.strip()
                if fr: stack.append(fr)

groups = collections.defaultdict(collections.Counter)
for thr, st in samples:
    if not st: continue
    leaf = st[0]
    key = None
    if "NoiseStack$Perlin" in leaf and any("maxSurfaceLevel" in f or "Aquifer" in f for f in st):
        key = "noise-aquifer"
    elif "NoiseStack$Perlin" in leaf and any("GeodeFeature" in f for f in st):
        key = "noise-geode"
    if key:
        groups[key][(thr, tuple(st))] += 1

for key, g in groups.items():
    print(f"\n##### {key}: {sum(g.values())} samples #####")
    for (thr, st), n in g.most_common(10):
        print(f"  --- x{n} [{thr}] leaf={st[0][:90]}")
        print("      TAIL (top of stack):")
        for fr in st[-12:][::-1]:
            print(f"        {fr}")
