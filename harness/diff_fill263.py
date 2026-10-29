#!/usr/bin/env python3
"""Compare oracle263_fill.txt (vanilla doFill) vs oracle263_fill_native.txt (Rust fill263)."""
import sys

def load(path):
    fills = {}
    marks = {}
    with open(path) as f:
        for line in f:
            p = line.split()
            if not p:
                continue
            if p[0] == "fill":
                key = (int(p[1]), int(p[2]), int(p[3]))
                fills[key] = int(p[4])
            elif p[0] == "mark":
                key = (int(p[1]), int(p[2]))
                marks.setdefault(key, set()).add(int(p[3]))
    return fills, marks

a_fills, a_marks = load(sys.argv[1])
b_fills, b_marks = load(sys.argv[2])

fa, fb = set(a_fills), set(b_fills)
missing = fa - fb   # oracle has, native doesn't
extra = fb - fa     # native has, oracle doesn't
diff_ids = [k for k in (fa & fb) if a_fills[k] != b_fills[k]]

print(f"cells: oracle={len(a_fills)} native={len(b_fills)}")
print(f"missing (oracle-only): {len(missing)}")
print(f"extra (native-only): {len(extra)}")
print(f"id mismatches: {len(diff_ids)}")

mark_ok = True
for key in set(a_marks) | set(b_marks):
    if a_marks.get(key, set()) != b_marks.get(key, set()):
        mark_ok = False
        oa, ob = a_marks.get(key, set()), b_marks.get(key, set())
        print(f"mark diff {key}: oracle-only={list(oa-ob)[:4]} native-only={list(ob-oa)[:4]}")
print(f"marks: {'MATCH' if mark_ok else 'MISMATCH'}")

def sample(cells, label, n=8):
    print(f"{label}: ", [(k, a_fills.get(k), b_fills.get(k)) for k in sorted(cells)[:n]])

if missing:
    sample(missing, "missing sample")
if extra:
    sample(extra, "extra sample")
if diff_ids:
    sample(diff_ids, "id-diff sample")

ok = not missing and not extra and not diff_ids and mark_ok
print("RESULT:", "BIT-EXACT" if ok else "DIVERGED")
sys.exit(0 if ok else 1)
