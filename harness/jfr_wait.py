#!/usr/bin/env python3
"""Aggregate jdk.NativeMethodSample + jdk.ThreadPark + jdk.JavaMonitorWait
from `jfr print` output (call with --stack-depth 64: the default 5-frame
truncation hides the wait sites).

The honest window is idle-dominated (~4% CPU seen via ExecutionSample), so
the Java execution samples cannot explain the ~25ms/chunk Paper-side wall.
Three blind spots, filled here:

  - jdk.NativeMethodSample : threads sitting in NATIVE code (our JNI fills,
    JVM internals). Profile period = 20ms -> 1 sample ~ 20ms native wall.
  - jdk.ThreadPark         : LockSupport.park waits >20ms with durations —
    CompletableFuture/chunk-future style waiting. THE latency attribution.
  - jdk.JavaMonitorWait    : Object.wait >10ms with durations.

Usage: jfr print --events jdk.NativeMethodSample,jdk.ThreadPark,jdk.JavaMonitorWait --stack-depth 64 file.jfr | jfr_wait.py
"""
import re
import sys
from collections import Counter, defaultdict

INFRA = ("watchdog", "notification thread", "vm thread", "gc thread")
BUSINESS = ("net.minecraft.", "ca.spottedleaf.", "com.planeta.", "org.bukkit.",
            "org.spigotmc.", "io.papermc.")

def is_infra(name):
    n = name.lower()
    return any(p in n for p in INFRA)

def business_frame(stack):
    for f in stack:
        if any(f.startswith(p) for p in BUSINESS):
            return f
    return stack[-1] if stack else "?"

cur_event = None
cur_thread = None
cur_dur_ms = 0.0
stack = []

native_count = Counter()          # thread -> native samples (1 ~ 20ms)
park_ms = Counter()               # thread -> total parked ms
park_count = Counter()            # thread -> park events
park_top_ms = defaultdict(Counter)  # thread -> leaf frame -> parked ms
event_type_ms = Counter()         # event type -> total ms

line_re_thread = re.compile(r'(?:eventThread|sampledThread|thread)\s*=\s*"([^"]+)"')
line_re_dur = re.compile(r'duration\s*=\s*([\d.]+)\s*(ns|us|ms|s)\b')
line_re_frame = re.compile(r'^\s+([\w$.]+\.[\w$<>]+\([^)]*\))(?: line: \d+)?$')

def to_ms(v, unit):
    return v * {"ns": 1e-6, "us": 1e-3, "ms": 1.0, "s": 1000.0}[unit]

def flush():
    global cur_event, cur_thread, cur_dur_ms, stack
    if cur_event is None:
        return
    t = cur_thread or "?"
    if cur_event == "jdk.NativeMethodSample":
        native_count[t] += 1
    elif cur_event in ("jdk.ThreadPark", "jdk.JavaMonitorWait") and not is_infra(t):
        d = cur_dur_ms
        park_ms[t] += d
        park_count[t] += 1
        event_type_ms[cur_event] += d
        if stack:
            park_top_ms[t][business_frame(stack)] += d
    cur_event = None
    cur_thread = None
    cur_dur_ms = 0.0
    stack.clear()

for line in sys.stdin:
    s = line.rstrip()
    if s.startswith("jdk."):
        flush()
        if s.endswith("{"):
            cur_event = s[:-1].strip()
        continue
    if cur_event is None:
        continue
    m = line_re_thread.search(s)
    if m and cur_thread is None:
        cur_thread = m.group(1)
        continue
    m = line_re_dur.search(s)
    if m:
        cur_dur_ms += to_ms(float(m.group(1)), m.group(2))
        continue
    m = line_re_frame.match(s)
    if m:
        stack.append(m.group(1).strip())
flush()

native_total = sum(native_count.values())
# NativeMethodSample has no stack: a sample means the thread was in native
# code — JNI compute for chunk-system threads, but IDLE epoll/select for
# selector/netty/console/rcon-listener threads. Group accordingly.
EPOLL_IDLE = ("selector", "netty", "console", "rcon listener", "httpclient")

def epoll_idle(t):
    n = t.lower()
    return any(p in n for p in EPOLL_IDLE)

work_native = {t: n for t, n in native_count.items() if not epoll_idle(t)}
print(f"native samples: total={native_total} (1 sample ~ 20ms; chunk threads = JNI compute, selector/netty/console = idle epoll)")
print("--- per-thread native wall (chunk-system first, est) ---")
chunk_native = {t: n for t, n in work_native.items()
                if "server thread" in t.lower() or t.lower().startswith(("paper common worker", "paper i/o worker", "paper light"))}
for t, n in sorted(chunk_native.items(), key=lambda kv: -kv[1]):
    print(f"{n:6d}  ~{n * 20 / 1000:6.2f}s  {t}")
other_native = {t: n for t, n in work_native.items() if t not in chunk_native}
for t, n in sorted(other_native.items(), key=lambda kv: -kv[1])[:6]:
    print(f"{n:6d}  ~{n * 20 / 1000:6.2f}s  {t}")

total_park = sum(park_ms.values())
print(f"parks: events={sum(park_count.values())} total_parked={total_park / 1000:.2f}s (ThreadPark>20ms + MonitorWait>10ms)")
def park_group(t):
    n = t.lower()
    if "server thread" in n or n.startswith(("paper common worker", "paper i/o worker", "paper light")):
        return 0
    return 1
print("--- parked ms, chunk-system threads first (the latency attribution) ---")
ordered = sorted(park_ms.items(), key=lambda kv: (park_group(kv[0]), -kv[1]))
for t, d in ordered[:12]:
    if is_infra(t):
        continue
    print(f"{d:9.0f}ms  n={park_count[t]:5d}  {t}")
print("--- top park sites (first business frame of the park stack) ---")
for t, d in ordered[:6]:
    if is_infra(t) or not park_top_ms[t]:
        continue
    print(f"[{t}]  total={d:.0f}ms")
    for f, fd in park_top_ms[t].most_common(6):
        print(f"  {fd:9.0f}ms  {f}")
