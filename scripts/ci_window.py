#!/usr/bin/env python3
"""Extract the window story from a CI 0_integration.txt log.

Usage: python3 ci_window.py <log.txt> [log2.txt ...]
Per lane (matched by 'tee e2e263X.log' markers) prints:
  honest window line (e2e/gen/native + NEW ramp) + stage[] line
  mspt digest, park/native attribution headers with first lines
"""
import re
import sys

for path in sys.argv[1:]:
    print("#" * 28, path)
    lane = "?"
    show_until_blank = 0
    for line in open(path, encoding="utf-8", errors="replace"):
        m = re.search(r"tee (e2e263\S*)\.log", line)
        if m:
            lane = m.group(1)
            continue
        if "honest window" in line:
            print(f"[{lane}] {line.strip()[:200]}")
            show_until_blank = 3  # also show the next lines (deco/fill263 prof)
            continue
        if "stage[terrain" in line:
            print(f"[{lane}]   {line.strip()[:170]}")
            show_until_blank = 0
            continue
        if show_until_blank > 0 and line.strip():
            # continuation lines of the window block (deco, fill263 prof...)
            print(f"[{lane}]   {line.strip()[:170]}")
            show_until_blank -= 1
            continue
        show_until_blank = 0
        if re.search(r"=== mspt|msbp|mspt:", line, re.I):
            print(f"[{lane}] {line.strip()[:160]}")
        elif re.match(r"\s*(median|avg|min) tick", line.strip()):
            print(f"[{lane}]   {line.strip()[:120]}")
        elif "native samples:" in line or "parks:" in line or "execution samples:" in line:
            print(f"[{lane}] {line.strip()[:160]}")
        elif re.match(r"\s+\d+\s+~\s*[\d.]+s\s+\S", line) or re.match(r"\s+\d+ms\s+n=", line):
            print(f"[{lane}]   {line.strip()[:140]}")
        elif "JFR started" in line or "JFR.start FAILED" in line:
            print(f"[{lane}] {line.strip()}")
