#!/usr/bin/env python3
"""Aggregate jdk.ExecutionSample events from `jfr print` output.

jfr print defaults to --stack-depth 5, which silently truncates stacks —
always call it with --stack-depth 64 (the harness does).

Frames are printed LEAF FIRST (verified against jdk.jfr consumer:
frame[0]=leaf, frame[last]=root). For each thread we print:
  - true leaf frames (what exactly was executing)
  - the first "business" frame walking up from the leaf (the gen-pipeline
    stage that owns the CPU; skips JDK internals like Unsafe.park)

Usage: jfr print --events jdk.ExecutionSample --stack-depth 64 file.jfr | jfr_top.py
"""
import re
import sys
from collections import Counter, defaultdict

INFRA = ("watchdog", "notification thread", "vm thread", "gc thread")
BUSINESS = ("net.minecraft.", "ca.spottedleaf.", "com.planeta.", "org.bukkit.",
            "org.spigotmc.", "it.unimi.", "io.papermc.")

def is_infra(name):
    n = name.lower()
    return any(p in n for p in INFRA)

def is_business(frame):
    return any(frame.startswith(p) for p in BUSINESS)

cur_thread = None
stack = []
thread_total = Counter()
thread_leaf = defaultdict(Counter)
thread_stage = defaultdict(Counter)

line_re_thread = re.compile(r'sampledThread\s*=\s*"([^"]+)"')
line_re_frame = re.compile(r'^\s+([\w$.]+/)?[\w$.]+\.[\w$<>]+\([^)]*\)(?: line: \d+)?$')
strip_re = re.compile(r'^\s+')

def flush():
    global cur_thread, stack
    if cur_thread is None:
        return
    thread_total[cur_thread] += 1
    if stack:
        thread_leaf[cur_thread][stack[0]] += 1
        for f in stack:
            if is_business(f):
                thread_stage[cur_thread][f] += 1
                break
    cur_thread = None
    stack.clear()

for raw in sys.stdin:
    line = raw.rstrip("\n")
    if line.startswith("jdk.ExecutionSample"):
        flush()
        continue
    m = line_re_thread.search(line)
    if m and cur_thread is None:
        cur_thread = m.group(1)
        continue
    body = strip_re.sub("", line)
    m = line_re_frame.match(line)
    if m and cur_thread is not None:
        stack.append(body)
flush()

total = sum(thread_total.values())
excluded = sum(v for k, v in thread_total.items() if is_infra(k))
# profile period for jdk.ExecutionSample is 10ms -> samples ~ 10ms Java CPU each
print(f"execution samples: total={total} excluded(infra)={excluded} profiled={total - excluded} (1 sample ~ 10ms java cpu)")
print("--- per-thread totals (profiled) ---")
for t, n in thread_total.most_common():
    if is_infra(t):
        continue
    print(f"{n:6d}  {100.0 * n / max(total - excluded, 1):5.1f}%  ~{n * 10 / 1000:.1f}s  {t}")
print("--- pipeline stage per thread (first business frame up the stack) ---")
for t, n in thread_total.most_common(8):
    if is_infra(t) or not thread_stage[t]:
        continue
    print(f"[{t}]")
    for f, c in thread_stage[t].most_common(8):
        print(f"  {c:5d}  {f}")
print("--- true leaves per thread ---")
for t, n in thread_total.most_common(8):
    if is_infra(t) or not thread_leaf[t]:
        continue
    print(f"[{t}]")
    for f, c in thread_leaf[t].most_common(8):
        print(f"  {c:5d}  {f}")
