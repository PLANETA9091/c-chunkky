#!/usr/bin/env python3
"""Parse `jfr print` text output (JDK 21 checkpoint format) -> thread-group
attribution + stage buckets + GC totals. Usage: analyze_jfr2.py <print.txt>"""
import re, sys, collections

TXT = sys.argv[1]

# jfr print format:
# jdk.ExecutionSample {
#   startTime = 07:07:08.123
#   objectClass = ...
#   weight = 3
#   stackTrace = [
#     some.Main.method line: 12
#     ...
#   ]
#   thread.name = "Worker-Main-1"
#   ...
# }
rx_event = re.compile(r'^jdk\.(ExecutionSample|NativeMethodSample|GCPhaseParallel) \{$')
rx_thread = re.compile(r'^\s+(?:sampledThread|thread\.name) = "([^"]+)"')
rx_dur = re.compile(r'^\s+duration = ([\d.]+) (\w+)')

samples = []          # (event, thread, frames)
gc_total_ns = collections.Counter()
cur = None
DUR = {'ns': 1e0, 'us': 1e3, 'ms': 1e6, 's': 1e9}
with open(TXT) as f:
    for line in f:
        m = rx_event.match(line)
        if m:
            cur = [m.group(1), None, []]
            continue
        if cur is None:
            continue
        m = rx_thread.match(line)
        if m and cur[1] is None:
            cur[1] = m.group(1); continue
        m = rx_dur.match(line)
        if m and cur[0] == 'GCPhaseParallel':
            gc_total_ns[cur[1] or 'gc'] += float(m.group(1)) * DUR[m.group(2)]
            cur = None; continue
        if line.startswith('}'):
            if cur and cur[0] in ('ExecutionSample', 'NativeMethodSample') and cur[1]:
                samples.append(tuple(cur))
            cur = None; continue
        if cur[2] is not None and 'stackTrace' not in line:
            s = line.strip()
            if s == '...' or s == ']':
                continue
            if re.match(r'^[\w$]+[\w$.<>/]*\.[\w$<>]+\(', s):
                cur[2].append(s)

# group threads
def tgroup(t):
    if t.startswith('Worker-Main') or 'Common Worker' in t or 'Worker' in t: return 'gen-worker'
    if t == 'Server thread': return 'Server thread'
    if 'GC' in t or 'G1' in t: return 'GC'
    if 'Common-Cleaner' in t or 'Reference Handler' in t or 'Finalizer' in t or 'Notification' in t: return 'jvm-housekeep'
    if 'Timer' in t or 'scheduler' in t.lower(): return 'sched'
    if 'IO' in t.upper() or 'File' in t or 'io' in t: return 'io'
    if 'Async' in t: return 'async'
    if 'main' == t: return 'main'
    return t[:24]

by_group = collections.Counter()
by_group_native = collections.Counter()
for ev, th, fr in samples:
    (by_group_native if ev == 'NativeMethodSample' else by_group)[tgroup(th)] += 1

print('== sample counts by thread group (exec / native) ==')
for g in sorted(set(by_group) | set(by_group_native), key=lambda x: -(by_group[x] + by_group_native[x])):
    print(f'{g:28s} exec={by_group[g]:5d} native={by_group_native[g]:5d}')

# gen-worker stage buckets (first-match, leaf->root)
BUCKETS = [
    ('JNI_fill(native)',     [r'^CrusstyGen Native', r'fillFromNoise', r'fill0', r'ChunkGenNative\.fill']),
    ('features',             [r'Feature', r'decorat', r'place', r'ChunkStep.*FEATURE', r'WorldGenRegion\.']),
    ('light',                [r'Light', r'Stars', r'skylight', r'SkyLight', r'BlockLight']),
    ('surface(native?)',     [r'buildSurface', r'SurfaceRule', r'SurfaceSys']),
    ('biomes',               [r'Biome', r'MultiNoise', r'Climate']),
    ('noise/java',           [r'NoiseChunk', r'DensityFunction', r'Perlin', r'NormalNoise', r'BlendedNoise']),
    ('heightmaps',           [r'Heightmap', r'heightmap']),
    ('serialization',        [r'Serializ', r'region', r'RegionFile', r'ChunkSerializ', r'PalettedContainer.*resize|PalettedContainer']),
    ('structures',           [r'Structure', r'Jigsaw', r'Piece']),
    ('chunk_sched/wait',     [r'ChunkTask', r'Scheduled', r'ChunkHolder', r'park', r'parkAndCheck', r'await', r'future', r'Future', r'CompletableFuture', r'waitFor']),
    ('GC-in-thread',         [r'GC\(', r'G1', r'garbage']),
    ('nbt',                  [r'Nbt', r'NBT', r'Tag']),
]
def bucket(fr):
    for name, pats in BUCKETS:
        for f in fr:
            for p in pats:
                if re.search(p, f): return name
    return 'other'

wk = collections.Counter()
wk_leaf = collections.Counter()
for ev, th, fr in samples:
    if tgroup(th) != 'gen-worker': continue
    b = bucket(fr)
    wk[b] += 1
    if fr: wk_leaf[fr[0][:110]] += 1

tot = sum(wk.values()) or 1
print(f'\n== gen-worker stage buckets (exec samples, total {tot}) ==')
for b, c in wk.most_common():
    print(f'{b:22s} {c:5d}  {100*c/tot:5.1f}%')
print('\n== gen-worker leaf top-15 ==')
for f, c in wk_leaf.most_common(15):
    print(f'{c:5d}  {f}')

# Server thread buckets
st = collections.Counter()
st_leaf = collections.Counter()
for ev, th, fr in samples:
    if tgroup(th) != 'Server thread': continue
    b = bucket(fr)
    st[b] += 1
    if fr: st_leaf[fr[0][:110]] += 1
tot = sum(st.values()) or 1
print(f'\n== Server thread stage buckets (exec samples, total {tot}) ==')
for b, c in st.most_common():
    print(f'{b:22s} {c:5d}  {100*c/tot:5.1f}%')
print('\n== Server thread leaf top-15 ==')
for f, c in st_leaf.most_common(15):
    print(f'{c:5d}  {f}')

# native method samples: which native frames
nat = collections.Counter()
for ev, th, fr in samples:
    if ev != 'NativeMethodSample': continue
    nat[tgroup(th)] += 1
print(f'\n== GC parallel phase total ==')
for g, ns in gc_total_ns.most_common():
    print(f'{g:20s} {ns/1e9:7.2f}s')
