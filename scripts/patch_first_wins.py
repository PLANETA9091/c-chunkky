#!/usr/bin/env python3
"""Patch native bins to vanilla first-pack-wins asset collection (v2, line-based)."""
import os, re

NATIVE = '/home/z/my-project/c-faster/native'
BIN = f'{NATIVE}/src/bin'

# 1) worldgen.rs helper (idempotent)
wg = open(f'{NATIVE}/src/worldgen.rs').read()
if 'collect_packs_first_wins' not in wg:
    anchor = """    pub fn insert(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.files.insert(path.into(), bytes);
    }"""
    assert anchor in wg
    wg = wg.replace(anchor, anchor + """

    /// First-wins insert (vanilla 1.21.8 server layering: the earlier pack in
    /// the enabled list overrides later packs on identical paths).
    pub fn insert_if_absent(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.files.entry(path.into()).or_insert(bytes);
    }""")
    helper = '''

// ---------------------------------------------------------------------------
// Pack collection (vanilla server layering)
// ---------------------------------------------------------------------------

fn collect_dir(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_dir(&p, base, out);
            } else if p.extension().map(|x| x == "json").unwrap_or(false) {
                if let Ok(rel) = p.strip_prefix(base) {
                    let rel = rel.to_string_lossy().replace('\\\\', "/");
                    let Some(stripped) = rel.strip_prefix("data/") else { continue };
                    if let Ok(bytes) = std::fs::read(&p) {
                        out.push((stripped, bytes));
                    }
                }
            }
        }
    }
}

/// Collect worldgen assets from a list of pack roots with VANILLA layering:
/// across packs the FIRST pack wins on identical paths (proven by the dptt
/// oracle SURFACE dump: Tectonic's surface_rule beats Terralith's); within a
/// pack the active overlay dir (`overlay.datapack`) overrides the root.
pub fn collect_packs_first_wins(
    roots: &[String],
    out: &mut Assets,
    keep: impl Fn(&str) -> bool,
) {
    for r in roots {
        let root = std::path::Path::new(r);
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        collect_dir(root, root, &mut files);
        let ov = root.join("overlay.datapack");
        if ov.is_dir() {
            let mut ovf: Vec<(String, Vec<u8>)> = Vec::new();
            collect_dir(&ov, &ov, &mut ovf);
            for (p, b) in ovf {
                files.retain(|(fp, _)| fp != &p);
                files.push((p, b));
            }
        }
        files.sort();
        for (path, bytes) in files {
            if keep(&path) {
                out.insert_if_absent(path, bytes);
            }
        }
    }
}
'''
    wg += helper
    open(f'{NATIVE}/src/worldgen.rs', 'w').write(wg)
    print("worldgen.rs: helper added")
else:
    print("worldgen.rs: already patched")


def find_block_end(lines, start):
    """start = index of the 'for root in ... {' line; return index of its closing '}' line."""
    depth = 0
    for i in range(start, len(lines)):
        depth += lines[i].count('{') - lines[i].count('}')
        if depth == 0 and i > start:
            return i
    raise ValueError("no end")


def patch_bin(name, full_prefix):
    p = f'{BIN}/{name}.rs'
    if not os.path.exists(p):
        print(f"{name}: missing"); return
    s = open(p).read()
    if 'collect_packs_first_wins' in s:
        print(f"{name}: already patched"); return
    lines = s.split('\n')
    # locate assets decl
    ai = next((i for i, l in enumerate(lines)
               if re.match(r"\s*let mut assets = (crussty_chunkgen::worldgen::)?Assets::new\(\);", l)), None)
    if ai is None:
        print(f"{name}: assets decl not found"); return
    # locate keep expr (last `let keep = ...;` before insert)
    ki = next((i for i, l in enumerate(lines) if re.match(r"\s*let keep = .+;$", l)), None)
    if ki is None:
        print(f"{name}: keep not found"); return
    keep = lines[ki].strip()[len('let keep = '):-1].strip()
    keep = re.sub(r"\bpath\.", "p.", keep)
    # locate the roots loop start: the nearest `for root in &...` AFTER assets decl
    fi = next((i for i in range(ai, len(lines))
               if re.match(r"\s*for root in &(roots|expanded) \{", lines[i])), None)
    if fi is None:
        print(f"{name}: roots loop not found"); return
    roots_var = lines[fi].split('for root in &')[1].split(' {')[0]
    fe = find_block_end(lines, fi)
    pre = 'crussty_chunkgen::worldgen::' if full_prefix else ''
    repl = [f"    {pre}collect_packs_first_wins(&{roots_var}, &mut assets, |p| {keep});"]
    # expanded pre-pass removal: if roots_var == 'expanded', also remove the
    # lines from 'let mut expanded' back up to and including its loop
    if roots_var == 'expanded':
        # find start of expanded block: first line mentioning 'expanded' before fi
        bi = next(i for i, l in enumerate(lines) if 'let mut expanded' in l)
        # include an optional comment lines directly above bi
        while bi > 0 and lines[bi - 1].strip().startswith('//'):
            bi -= 1
        lines[bi:fe + 1] = repl
    else:
        lines[fi:fe + 1] = repl
    s = '\n'.join(lines)
    s = s.replace("fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {",
                  "#[allow(dead_code)]\nfn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {")
    s = s.replace("fn collect(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {",
                  "#[allow(dead_code)]\nfn collect(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {")
    open(p, 'w').write(s)
    print(f"{name}: patched roots=&{roots_var} keep={keep[:60]}...")


for name in ["sfcmp", "biomefill", "climprobe", "dpcmp", "dpload", "filldump",
             "yprobe", "dfprobe", "batchref", "phases"]:
    patch_bin(name, full_prefix=(name == "phases"))
