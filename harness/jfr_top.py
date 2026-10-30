#!/usr/bin/env python3
"""Aggregate jdk.ExecutionSample events from `jfr print` output.

Groups samples by sampled thread, excludes infrastructure spinners
(WatchdogThread) that drown the profile, and prints:
  - per-thread sample totals (where the window's wall went)
  - the top leaf frames per thread (who owns the CPU)

Usage: jfr print --events jdk.ExecutionSample file.jfr | jfr_top.py
"""
import re
import sys
from collections import Counter, defaultdict

EXCLUDE_THREADS = {"WatchdogThread", "Notification Thread", "VM Thread", "GC Thread#0"}

cur_thread = None
stack = []
thread_total = Counter()
thread_top = defaultdict(Counter)

line_re_thread = re.compile(r'sampledThread\s*=\s*"([^"]+)"')
line_re_frame = re.compile(r'^\s+([\w$.]+\.[\w$<>]+\([^)]*\))(?: line: \d+)?$')

def flush():
    global cur_thread, stack
    if cur_thread is None:
        return
    thread_total[cur_thread] += 1
    if stack:
        thread_top[cur_thread][stack[-1]] += 1
    cur_thread = None
    stack.clear()

for line in sys.stdin:
    if line.startswith("jdk.ExecutionSample"):
        flush()
        continue
    m = line_re_thread.search(line)
    if m and cur_thread is None:
        cur_thread = m.group(1)
        continue
    m = line_re_frame.match(line.rstrip())
    if m and cur_thread is not None:
        stack.append(m.group(1).strip())
flush()

total = sum(thread_total.values())
excluded = sum(v for k, v in thread_total.items() if k in EXCLUDE_THREADS)
print(f"execution samples: total={total} excluded(infra)={excluded} profiled={total - excluded}")
print("--- per-thread totals (profiled) ---")
for t, n in thread_total.most_common():
    if t in EXCLUDE_THREADS:
        continue
    print(f"{n:6d}  {100.0 * n / max(total - excluded, 1):5.1f}%  {t}")
print("--- top leaf frames per thread ---")
for t, n in thread_total.most_common(8):
    if t in EXCLUDE_THREADS or not thread_top[t]:
        continue
    print(f"[{t}]")
    for f, c in thread_top[t].most_common(8):
        print(f"  {c:5d}  {f}")
