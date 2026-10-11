#!/usr/bin/env python3
"""Judge gate: fail if block parity regressed vs the recorded gate.

Usage: judge_gate.py <diffpct> <gatepct> <world>
Reads diffpct from diff_dumps.py OVERALL line parsing done by the caller.

Exit 0 if diffpct <= gatepct (parity did not get worse), else 1.
Also prints matched % for convenience.
"""
import sys


def main() -> int:
    diff = float(sys.argv[1])
    gate = float(sys.argv[2])
    world = sys.argv[3] if len(sys.argv) > 3 else "?"
    matched = 100.0 - diff
    print(f"JUDGE[{world}]: matched {matched:.3f}% (diff {diff:.3f}%, gate {gate:.3f}%)")
    if diff > gate + 1e-9:
        print(f"JUDGE[{world}]: FAIL — diff {diff:.3f}% above gate {gate:.3f}%")
        return 1
    print(f"JUDGE[{world}]: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
