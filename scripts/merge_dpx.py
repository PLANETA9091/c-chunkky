#!/usr/bin/env python3
"""Replicate the production datapack merge (CrusstyChunkGenerator.loadPack):
pack root data/ + active overlays (pack.mcmeta top-level overlays.entries whose
format range covers PACK_FORMAT), packs applied in name order, later overrides.
Usage: merge_dpx.py <out_dir> <pack_format> <zip...>"""
import json
import os
import sys
import zipfile


def active_overlays(zf):
    text = zf.read('pack.mcmeta').decode('utf-8', 'replace')
    mc = json.loads(text)
    out = []
    for e in mc.get('overlays', {}).get('entries', []):
        lo, hi = None, None
        if 'formats' in e and isinstance(e['formats'], list) and len(e['formats']) == 2:
            lo, hi = e['formats']
        if 'min_format' in e:
            lo = e['min_format']
        if 'max_format' in e:
            hi = e['max_format']
        if lo is not None and hi is not None and lo <= PACK_FORMAT <= hi:
            out.append(e['directory'])
    return out


def main():
    global PACK_FORMAT
    out_dir = sys.argv[1]
    PACK_FORMAT = int(sys.argv[2])
    packs = sys.argv[3:]
    merged = {}
    for pack in packs:
        zf = zipfile.ZipFile(pack)
        names = sorted(zf.namelist())
        pack_map = {}
        for n in names:
            if n.endswith('/'):
                continue
            if n.startswith('data/'):
                pack_map[n[len('data/'):]] = zf.read(n)
        for ov in active_overlays(zf):
            prefix = ov + '/data/'
            for n in names:
                if n.endswith('/') or not n.startswith(prefix):
                    continue
                pack_map[n[len(prefix):]] = zf.read(n)
        pack_map.pop('pack.mcmeta', None)
        merged.update(pack_map)
    total = 0
    for rel, data in merged.items():
        p = os.path.join(out_dir, 'data', rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, 'wb') as f:
            f.write(data)
        total += 1
    print(f'merged {total} files -> {out_dir}/data')


if __name__ == '__main__':
    main()
