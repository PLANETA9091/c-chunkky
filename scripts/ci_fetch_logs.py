#!/usr/bin/env python3
"""Download judge logs artifact from a CI run and extract diff% verdicts."""
import io
import json
import re
import subprocess
import sys
import zipfile

import requests

REPO = "PLANETA9091/c-faster"


def token() -> str:
    url = subprocess.check_output(
        ["git", "remote", "get-url", "origin"], text=True
    ).strip()
    m = re.match(r"https://[^:]+:(.+)@github\.com", url)
    return m.group(1)


def api(path: str):
    t = token()
    url = f"https://api.github.com/repos/{REPO}/{path}"
    r = requests.get(url, headers={
        "Authorization": f"token {t}",
        "Accept": "application/vnd.github+json",
        "User-Agent": "ci-verdict-reader",
    }, timeout=120)
    r.raise_for_status()
    return r.content


def main():
    run_id = int(sys.argv[1])
    want = sys.argv[2:] or ["judge_vanilla.log", "judge_datapack.log", "judge_dptt.log"]
    d = json.loads(api(f"actions/runs/{run_id}/artifacts").decode())
    for a in d.get("artifacts", []):
        print(f"artifact: {a['name']} size={a['size_in_bytes']} expired={a['expired']}")
        z = zipfile.ZipFile(io.BytesIO(api(f"actions/artifacts/{a['id']}/zip")))
        for name in z.namelist():
            base = name.split("/")[-1]
            if base in want or name in want:
                data = z.read(name).decode("utf-8", "replace")
                out = f"/home/z/my-project/c-faster/reference/ci_logs/run{run_id}_{base}"
                with open(out, "w") as f:
                    f.write(data)
                print(f"  saved {name} -> {out} ({len(data)} bytes)")
                # grep the verdicts
                for line in data.splitlines():
                    if "OVERALL:" in line or "diff% =" in line or "GATE" in line or "gate" in line.lower() and "PASS" in line or "FAIL" in line:
                        print(f"    | {line.strip()[:160]}")


if __name__ == "__main__":
    main()
