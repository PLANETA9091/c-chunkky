#!/usr/bin/env python3
"""analyze_jfr.py <jfr_print.txt> — aggregate jdk.ExecutionSample /
jdk.NativeMethodSample events from `jfr print` text into a stage profile.

Buckets are attributed by the FIRST matching frame walking the stack top-down
(leaf -> root). NativeMethodSample events catch threads resident in native
code (the Rust fill via JNI does not appear in ExecutionSample).
"""
import re
import sys
from collections import Counter, defaultdict

EV_RE = re.compile(r"^jdk\.(ExecutionSample|NativeMethodSample) \{")
THREAD_RE = re.compile(r'sampledThread = "([^"]*)" \(javaThreadId = (\d+)\)')
FRAME_RE = re.compile(r"^    (.+)$")

# attribution buckets: name -> regex over a single frame
BUCKETS = [
    ("native_fill_jni", re.compile(r"crussty.*\(")),
    ("plugin_java", re.compile(r"^com\.planeta\.crussty")),
    ("noise_java", re.compile(r"levelgen\.(NoiseBasedChunkGenerator|NoiseChunk|NoiseRouter|DensityFunctions|BlendedNoise|ImprovedNoise|PerlinNoise|NormalNoise|SurfaceSystem)|levelgen\.surface")),
    ("carver", re.compile(r"CarvingCache|Carver")),
    ("structures", re.compile(r"levelgen\.structure|Beardifier")),
    ("features", re.compile(r"levelgen\.feature|block\.grow|PlacementModifier|worldgen\.placement")),
    ("biomes", re.compile(r"[Bb]iome|MultiNoise|Climate\.")),
    ("light", re.compile(r"[Ll]ight[Ll]ight|LightEngine|LayerLight")),
    ("serialize", re.compile(r"ChunkSerializer|NbtIo|nbt\.|TagWriter|StorageIoWorker|IOWorker")),
    ("chunk_sys", re.compile(r"ChunkTaskScheduler|ChunkHolder|ChunkStep|ChunkMap|NewChunkHolder|ChunkGeneratorTask")),
    ("protocol", re.compile(r"io\.netty|ServerGamePacketListener|connection\.")),
    ("jvm", re.compile(r"^java\.(util|lang|io|nio)|^it\.unimi|^org\.slf4j|^java\.util\.concurrent")),
]

def bucket_of(stack):
    for frame in stack:
        for name, rx in BUCKETS:
            if rx.search(frame):
                return name, frame
    return "other", stack[0] if stack else "?"

def thread_group(name):
    if name.startswith("Worker-Main") or name.startswith("Paper Common Worker") \
            or name.startswith("Paper Async Worker") or name.startswith("Paper RegionWorker"):
        return "Worker-Main"
    if name == "Server thread":
        return "Server thread"
    if name.startswith("IO-Worker") or name.startswith("Paper I/O") or "I/O" in name:
        return "IO-Worker"
    if name.startswith("Craft Scheduler"):
        return "Craft Scheduler"
    return None  # ignore JVM-internal for the report

def parse(path):
    events = []
    cur = None
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            m = EV_RE.match(line.rstrip("\n"))
            if m:
                cur = {"ev": m.group(1), "thread": "?", "stack": []}
                events.append(cur)
                continue
            if cur is None:
                continue
            tm = THREAD_RE.match(line.strip()) if line.startswith("  sampledThread") else None
            if tm:
                cur["thread"] = tm.group(1)
                continue
            if line.startswith("    "):
                f = line[4:].rstrip("\n")
                if f and f != "...":
                    cur["stack"].append(f)
    return events

def main(path):
    events = parse(path)
    print(f"events total: {len(events)}")
    by_group_ev = Counter()
    for e in events:
        g = thread_group(e["thread"])
        if g:
            by_group_ev[(g, e["ev"])] += 1
    print("\n== samples per thread group ==")
    for (g, ev), n in sorted(by_group_ev.items()):
        print(f"{g:18s} {ev:20s} {n}")

    # gen threads only
    gen = [e for e in events if thread_group(e["thread"]) in ("Worker-Main", "Server thread")]

    print("\n== leaf frames top-20 (Worker-Main + Server thread, ExecutionSample) ==")
    leaves = Counter(e["stack"][0] if e["stack"] else "?" for e in gen if e["ev"] == "ExecutionSample")
    for f, n in leaves.most_common(20):
        print(f"{n:6d}  {f[:110]}")

    print("\n== stage buckets (Worker-Main + Server thread) ==")
    agg = defaultdict(Counter)
    example = {}
    for e in gen:
        b, frame = bucket_of(e["stack"])
        agg[b][e["ev"]] += 1
        if b not in example:
            example[b] = (e["thread"], e["stack"][:4])
    tot = sum(sum(c.values()) for c in agg.values())
    for b, c in sorted(agg.items(), key=lambda kv: -sum(kv[1].values())):
        n = sum(c.values())
        print(f"{b:18s} {n:6d} ({100.0*n/max(tot,1):5.1f}%)  exec={c['ExecutionSample']} native={c['NativeMethodSample']}")
        t, st = example[b]
        print(f"    ex[{t}]: " + " <- ".join(s[:70] for s in st))

    print("\n== NativeMethodSample leaf top-10 for Worker-Main ==")
    nl = Counter(e["stack"][0] if e["stack"] else "(no java frame)" for e in events
                 if e["ev"] == "NativeMethodSample" and e["thread"].startswith("Worker-Main"))
    for f, n in nl.most_common(10):
        print(f"{n:6d}  {f[:110]}")

if __name__ == "__main__":
    main(sys.argv[1])
