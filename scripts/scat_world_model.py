#!/usr/bin/env python3
"""Full scattered_ore flow model over the REAL undecorated world (from the
vanilla A-dumps, ores reverted to base stone). Reproduces the draw stream
(xoro core = WgrRef formulas), the rule tests against the dump world, the
overlay re-reads, and prints the spos rounds per copy. Compare with the
native [scat]/[spos] trace and the A/B dump blocks."""
import struct

MASK64 = (1 << 64) - 1
SILVER = 7640891576956012809
GOLDEN = -7046029254386353131 & MASK64

def mix_stafford13(s):
    s &= MASK64
    s = ((s ^ (s >> 30)) * (-4658895280553007687 & MASK64)) & MASK64
    s = ((s ^ (s >> 27)) * (-7723592293110705685 & MASK64)) & MASK64
    return (s ^ (s >> 31)) & MASK64

def rotl(x, k):
    x &= MASK64
    return ((x << k) | (x >> (64 - k))) & MASK64

class Xoro:
    def __init__(self):
        self.lo = self.hi = 0
    def set_seed(self, seed):
        seed &= MASK64
        l = (seed ^ SILVER) & MASK64
        l1 = (l + GOLDEN) & MASK64
        self.lo = mix_stafford13(l)
        self.hi = mix_stafford13(l1)
        if (self.lo | self.hi) == 0:
            self.lo, self.hi = GOLDEN, SILVER
    def next_long(self):
        l, l1 = self.lo, self.hi
        l2 = (rotl((l + l1) & MASK64, 17) + l) & MASK64
        l1 ^= l
        self.lo = rotl(l, 49) ^ l1 ^ ((l1 << 21) & MASK64)
        self.hi = rotl(l1, 28)
        return l2 & MASK64

def s32(x):
    return x - (1 << 32) if x >= (1 << 31) else x

def s64(x):
    x &= MASK64
    return x - (1 << 64) if x >> 63 else x

class Wgr:
    def __init__(self):
        self.x = Xoro()
        self.count = 0
    def set_seed(self, seed):
        self.x.set_seed(seed & MASK64)
    def next(self, bits):
        self.count += 1
        return (self.x.next_long() >> (64 - bits)) if bits < 64 else self.x.next_long()
    def wr_next_long(self):
        # BitRandomSource.nextLong: ((long)next(32) << 32) + next(32) — both
        # next(32) are SIGNED ints (WorldgenRandom.next feeds from xoro).
        hi = s32(self.next(32))
        lo = s32(self.next(32))
        return ((hi << 32) + lo) & MASK64
    def next_int(self, bound):
        if (bound & -bound) == bound:
            return (bound * self.next(31)) >> 31
        i = self.next(31)
        r = i % bound
        while i - r + (bound - 1) < 0:
            i = self.next(31)
            r = i % bound
        return r
    def next_float(self):
        return self.next(24) * 5.9604645e-8

def set_decoration_seed(w, level_seed, bx, bz):
    w.set_seed(level_seed)
    a = s64(w.wr_next_long()) | 1
    b = s64(w.wr_next_long()) | 1
    s = (((bx * a + bz * b) & MASK64) ^ (level_seed & MASK64)) & MASK64
    w.set_seed(s)
    return s

def set_feature_seed(w, deco, gid, step):
    w.set_seed((deco + gid + 10000 * step) & MASK64)

def round_java(v):
    import math
    return math.floor(v + 0.5) if v + 0.5 >= 0 else -math.floor(-(v + 0.5))

ST = {0:"stone",1:"deepslate",2:"granite",3:"diorite",4:"andesite",5:"tuff",6:"copper_ore",
      7:"deepslate_copper_ore",8:"air",9:"bedrock",14:"other"}

def main():
    data = open("/tmp/world99.bin", "rb").read()
    X0,X1,Z0,Z1,Y0,Y1 = struct.unpack_from("<6i", data, 0)
    cells = data[24:]
    def world(x,y,z):
        if not (X0<=x<=X1 and Z0<=z<=Z1 and Y0<=y<=Y1):
            return "stone" if y >= 0 else "deepslate"
        return ST[cells[(y-Y0)*(Z1-Z0+1)*(X1-X0+1) + (z-Z0)*(X1-X0+1) + (x-X0)]]

    overlay = {}
    level_seed = 3257840388504953787
    bx, bz, step, y_lo = 1584, 1568, 6, -16
    w = Wgr()
    deco = set_decoration_seed(w, level_seed, bx, bz)
    set_feature_seed(w, deco, 0, step)  # gid 0, single feature pack
    for p in range(4):
        px = bx + w.next_int(16)
        pz = bz + w.next_int(16)
        py = w.next_int(41) + y_lo
        n = w.next_int(8)
        print(f"c{p}=({px},{py},{pz},{n})[")
        for i in range(n):
            mag = min(i, 7)
            dx = dy = dz = 0
            fbits = []
            for a_i in range(3):
                f1, f2 = w.next_float(), w.next_float()
                v = (f1 - f2) * mag
                val = int(math.floor(v + 0.5)) if (v + 0.5) >= 0 else -int(math.floor(-(v + 0.5)))
                fbits.append(f"{f1:.6f}/{f2:.6f}->{val}")
                if a_i == 0: dx = val
                elif a_i == 1: dy = val
                else: dz = val
            sx, sy, sz = px+dx, py+dy, pz+dz
            key = (sx, sy, sz)
            st = overlay.get(key, world(sx, sy, sz))
            if st == "stone":
                rf = w.next_float()
                hit = rf < 0.7
                print(f"  spos{i} {key} st={st} rule f={rf:.6f} hit={hit}")
                if hit: overlay[key] = "copper_ore"
            elif st == "deepslate":
                print(f"  spos{i} {key} st=deepslate -> deepslate_copper_ore")
                overlay[key] = "deepslate_copper_ore"
            else:
                print(f"  spos{i} {key} st={st} -> miss")
        print("]")

import math
main()
