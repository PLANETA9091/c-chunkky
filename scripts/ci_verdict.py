#!/usr/bin/env python3
"""CI verdict reader — pulls run conclusions for given run-ids or head SHAs.
Token is read from the git remote URL INSIDE the process; never printed."""
import json
import re
import subprocess
import sys
import urllib.request

REPO = "PLANETA9091/c-faster"


def token() -> str:
    url = subprocess.check_output(
        ["git", "remote", "get-url", "origin"], text=True
    ).strip()
    m = re.match(r"https://[^:]+:(.+)@github\.com", url)
    return m.group(1)


def api(path: str, params: dict | None = None) -> dict:
    t = token()
    url = f"https://api.github.com/repos/{REPO}/{path}"
    if params:
        url += "?" + "&".join(f"{k}={v}" for k, v in params.items())
    req = urllib.request.Request(url, headers={
        "Authorization": f"token {t}",
        "Accept": "application/vnd.github+json",
        "User-Agent": "ci-verdict-reader",
    })
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.loads(r.read().decode())


def jobs(run_id: int) -> list:
    d = api(f"actions/runs/{run_id}/jobs", {"per_page": "100"})
    return d.get("jobs", [])


def run_info(run_id: int) -> dict:
    d = api(f"actions/runs/{run_id}")
    return {
        "id": d["id"],
        "head_sha": d["head_sha"][:7],
        "head_branch": d["head_branch"],
        "event": d["event"],
        "status": d["status"],
        "conclusion": d["conclusion"],
        "created": d["created_at"],
        "title": (d.get("display_title") or "")[:80],
    }


def main():
    args = sys.argv[1:]
    if not args:
        # last 30 completed runs on main
        d = api("actions/runs", {"per_page": "30", "branch": "main"})
        for r in d.get("workflow_runs", []):
            print(f"{r['id']}  {r['run_started_at']}  {r['head_sha'][:7]}  "
                  f"{r['status']}/{r['conclusion']}  {(r.get('display_title') or '')[:60]}")
        return
    for a in args:
        if a.isdigit():
            rid = int(a)
            info = run_info(rid)
            print(f"=== run {rid}: {info['head_sha']} [{info['head_branch']}] "
                  f"{info['status']}/{info['conclusion']} {info['created']}")
            print(f"    title: {info['title']}")
            for j in jobs(rid):
                print(f"    job: {j['name']:<22} {j['status']}/{j['conclusion']}  "
                      f"id={j['id']}  started={j['started_at']}")
        else:
            # sha prefix -> find runs
            d = api("actions/runs", {"per_page": "40"})
            for r in d.get("workflow_runs", []):
                if r["head_sha"].startswith(a):
                    print(f"{r['id']}  {r['run_started_at']}  {r['head_sha'][:7]}  "
                          f"{r['status']}/{r['conclusion']}  {(r.get('display_title') or '')[:60]}")


if __name__ == "__main__":
    main()
