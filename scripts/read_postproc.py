#!/usr/bin/env python3
"""Read PostProcessing lists from a vanilla region file (ground truth marks).

Region format: 4096-byte location table (3-byte sector offset + 1-byte count),
chunk payload: 4-byte length, 1-byte compression (2=zlib), zlib NBT.
Section NBT "PostProcessing": ListTag of 16 entries; entry k (y-layer) holds
packed locals (yLocal<<8 | z<<4 | x) as ints (or shorts/bytes depending on
DataVersion).
"""
import struct, zlib, sys, io
from pathlib import Path


class NBT:
    def __init__(self, data: bytes):
        self.b = data
        self.p = 0
        root_type, = struct.unpack_from(">b", data, self.p); self.p += 1
        if root_type != 10:
            raise ValueError("root not compound")
        self._string()  # root name (usually "")
        self.root = self._payload(10)[0]

    def _u1(self): v = self.b[self.p]; self.p += 1; return v
    def _s2(self): v = struct.unpack_from(">h", self.b, self.p)[0]; self.p += 2; return v
    def _u2(self): v = struct.unpack_from(">H", self.b, self.p)[0]; self.p += 2; return v
    def _s4(self): v = struct.unpack_from(">i", self.b, self.p)[0]; self.p += 4; return v
    def _s8(self): v = struct.unpack_from(">q", self.b, self.p)[0]; self.p += 8; return v
    def _f4(self): v = struct.unpack_from(">f", self.b, self.p)[0]; self.p += 4; return v
    def _f8(self): v = struct.unpack_from(">d", self.b, self.p)[0]; self.p += 8; return v

    def _string(self):
        n = self._u2()
        s = self.b[self.p:self.p+n].decode("utf-8", "replace"); self.p += n
        return s

    def _payload(self, t):
        if t == 1: return self._u1(),
        if t == 2: return self._u2(),
        if t == 3: return self._s4(),
        if t == 4: return self._s8(),
        if t == 5: return self._f4(),
        if t == 6: return self._f8(),
        if t == 7:
            n = self._s4(); v = self.b[self.p:self.p+n]; self.p += n; return v,
        if t == 8: return self._string(),
        if t == 9:
            it = self._u1(); n = self._s4()
            lst = []
            for _ in range(n):
                v = self._payload(it)[0]
                lst.append(v)
            return lst,
        if t == 10:
            d = {}
            while True:
                ct = self._u1()
                if ct == 0:
                    break
                name = self._string()
                d[name] = self._payload(ct)[0]
            return d,
        if t == 11:
            n = self._s4()
            v = list(struct.unpack_from(f">{n}i", self.b, self.p)); self.p += 4*n; return v,
        if t == 12:
            n = self._s4()
            v = list(struct.unpack_from(f">{n}q", self.b, self.p)); self.p += 8*n; return v,
        raise ValueError(f"bad tag {t} at {self.p}")


def read_region(path: Path):
    data = path.read_bytes()
    out = {}
    for i in range(1024):
        off = struct.unpack_from(">I", b"\0" + data[i*4:i*4+3])[0]
        cnt = data[i*4+3]
        if off == 0 or cnt == 0:
            continue
        base = off * 4096
        ln = struct.unpack_from(">I", data, base)[0]
        comp = data[base+4]
        payload = data[base+5:base+4+ln]
        if comp == 2:
            raw = zlib.decompress(payload)
        elif comp == 1:
            import gzip
            raw = gzip.decompress(payload)
        else:
            raise ValueError(f"comp {comp}")
        nbt = NBT(raw)
        dat = nbt.root.get("", nbt.root)  # root compound wraps data under ""
        cx = dat.get("xPos"); cz = dat.get("zPos")
        out[(cx, cz)] = dat
    return out


def decode_postprocessing(root):
    """yields (x, y, z) world coords of marked positions."""
    cx = root.get("xPos"); cz = root.get("zPos")
    for sec in root.get("sections", []):
        sy = sec.get("Y")
        pp = sec.get("PostProcessing")
        if not pp:
            continue
        for k, arr in enumerate(pp):
            if arr is None:
                continue
            vals = list(arr) if isinstance(arr, (list, bytes)) else [arr]
            # IntArrayTag -> list of ints; ByteArrayTag -> bytes (short pairs BE)
            if isinstance(arr, bytes):
                ints = list(struct.unpack(f">{len(arr)//2}H", arr)) if len(arr) % 2 == 0 else []
            else:
                ints = list(arr)
            for packed in ints:
                if packed == 0:
                    # packed 0 could be a real mark at x=z=yLocal=0 — rare; keep
                    pass
                x = packed & 15
                z = (packed >> 4) & 15
                yl = (packed >> 8) & 15
                y = sy * 16 + k * 16 + yl
                yield (cx * 16 + x, y, cz * 16 + z)


if __name__ == "__main__":
    regdir = Path(sys.argv[1] if len(sys.argv) > 1 else "/home/z/my-project/vanref/vanref/region")
    want = {(3, -2), (3, -3), (2, -2), (2, -3)}
    for f in sorted(regdir.glob("r.*.*.mca")):
        parts = f.stem.split(".")
        rx, rz = int(parts[1]), int(parts[2])
        # chunks (2..3, -3..-2) -> regions r.0.-1 (cx 2..3 in 0..0? cx>>5)
        chunks = read_region(f)
        for (cx, cz), root in chunks.items():
            if (cx, cz) in want:
                marks = sorted(decode_postprocessing(root), key=lambda t: (t[1], t[0], t[2]))
                print(f"chunk {cx},{cz}: {len(marks)} postprocessing marks")
                for m in marks:
                    print(f"  {m[0]} {m[1]} {m[2]}")
