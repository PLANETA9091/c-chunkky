#!/usr/bin/env python3
"""cont-143a2: reproduce window-time java attribution from jfr_print.txt.

Window = startTime >= 17:11:00 (boot Done 17:10:58). 2 windows x 144 inside
chunks = 288; 1 sample ~ 10ms java CPU => ms/ch = n/288*10.

Prints:
  1. per-thread top leaf frames in window
  2. full stacks for target leafs (NoiseStack$Perlin, stream/spliterator frames,
     layoutBulk, save path) walked up to first net.minecraft/com.planeta frame
"""
import re, sys, collections

PATH = "/home/z/my-project/c-faster/reference/jfr_cont143/jfr_print.txt"
WIN = "17:11:00"

rec_re = re.compile(r'startTime = (\d\d:\d\d:\d\d\.\d+)')
thr_re = re.compile(r'sampledThread = "([^"]+)"')

samples = []  # (time, thread, [frames leaf-first])
with open(PATH) as f:
    cur_t = cur_thr = None
    in_stack = False
    stack = []
    for line in f:
        if line.startswith("jdk.ExecutionSample"):
            cur_t = cur_thr = None; stack = []; in_stack = False
            continue
        m = rec_re.search(line)
        if m and cur_t is None:
            cur_t = m.group(1); continue
        m = thr_re.search(line)
        if m and cur_thr is None:
            cur_thr = m.group(1); continue
        if line.startswith("  stackTrace = ["):
            in_stack = True; continue
        if in_stack:
            if line.startswith("  ]"):
                in_stack = False
                if cur_t and cur_t >= WIN:
                    samples.append((cur_t, cur_thr, stack))
                stack = []
            else:
                fr = line.strip()
                if fr:
                    stack.append(fr)

print(f"total samples in file-window (>= {WIN}): {len(samples)}")

by_thread = collections.Counter(s[1] for s in samples)
print("\n=== samples per thread (window) ===")
for t, n in by_thread.most_common():
    print(f"  {t}: {n}")

# leaf = first frame
leaf = collections.Counter((s[1], s[2][0]) for s in samples if s[2])
print("\n=== top leaf frames (thread, leaf) x count ===")
for (t, l), n in leaf.most_common(60):
    print(f"  {n:4d}  [{t}]  {l}")

msch = lambda n: n / 288 * 10.0

TARGETS = [
    ("NoiseStack$Perlin", "noisestack"),
    ("ReferencePipeline", "stream"),
    ("copyInto", "stream"),
    ("Spliterator", "stream"),
    ("forEachRemaining", "stream"),
    ("layoutBulk", "layoutbulk"),
    ("CompoundTag.store", "save"),
    ("PaletteReadoutCache", "save"),
    ("ListTag.getIdToWrite", "save"),
    ("PalettedContainer", "save"),
    ("GeodeFeature", "geode"),
    ("performLightIncrease", "light"),
    ("ForwardLogHandler", "log"),
    ("StaticCache2D", "staticcache"),
    ("CarverDriver", "carver"),
]

def walk_owner(stack):
    """first net.minecraft / com.planeta frame walking up"""
    for fr in stack:
        if "net.minecraft" in fr or "com.planeta" in fr:
            return fr
    return "(none)"

seen_groups = collections.defaultdict(set)  # tag -> set of full stacks
counts = collections.Counter()
for tag_key, tag in TARGETS:
    cnt = 0
    for t, thr, stack in samples:
        if not stack: continue
        if any(tag_key in fr for fr in stack[:1]):  # match LEAF only for precise attribution
            cnt += 1
            counts[(tag, thr, stack[0], walk_owner(stack))] += 1
    if cnt:
        print(f"\n### leaf-target '{tag_key}': {cnt} samples ~ {msch(cnt):.2f} ms/ch")

print("\n=== leaf-target attribution (tag, thread, leaf -> first owner frame) ===")
for (tag, thr, lf, owner), n in sorted(counts.items(), key=lambda kv: -kv[1]):
    print(f"  {n:4d} ~{msch(n):.2f}ms/ch  [{thr}] {lf}\n        owner: {owner}")

# full stacks for the noisy ones: dump distinct stacks per tag, compact
print("\n" + "=" * 80)
print("=== DISTINCT FULL STACKS per interesting tag ===")
for tag_key, tag in TARGETS:
    stacks = collections.Counter()
    for t, thr, stack in samples:
        if stack and tag_key in stack[0]:
            stacks[(thr, tuple(stack))] += 1
    if not stacks: continue
    print(f"\n##### {tag_key} ({sum(stacks.values())} samples) #####")
    for (thr, st), n in stacks.most_common(6):
        print(f"  --- x{n} [{thr}] ---")
        depth = len(st)
        for i, fr in enumerate(st):
            mark = ""
            if "net.minecraft" in fr or "com.planeta" in fr:
                mark = "   << OWNER"
            print(f"    {i:2d} {fr}{mark}")
            if mark and i > 25:
                print(f"    ... ({depth-i-1} more frames above owner)")
                break
