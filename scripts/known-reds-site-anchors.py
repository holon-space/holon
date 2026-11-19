#!/usr/bin/env python3
"""Every site anchor of a known-red row rejects the row's own wording from another site.

For each known-red row with a `Site` anchor, this takes the row's evidence log, moves every panic location in it
somewhere the anchor rejects, and expects the classifier to fail with
`cross-family match: <key>` (exit 1), or to name another row that accepts the moved location (an unanchored row of the same wording) and never this one. A row whose evidence has no such location is listed and must be on EXEMPT
with a reason. A row that gains an anchor is covered without editing this file.

Run: `python3 scripts/known-reds-site-anchors.py [--repo <root>] [--only <key> ...]`.
`--repo` names a tree whose `scripts/` and registry are classified; `--anchored-in` a registry that decides which
rows must be covered, so a tree with a dropped anchor fails instead of shrinking the list.
"""
import argparse
import re
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

PANIC_HEAD = re.compile(r"panicked at \S+?:\d+:\d+:")
MOVED = "crates/zz-moved-site/src/moved.rs:1:1"
# Rows whose evidence cannot say where the panic is raised, with the reason.
EXEMPT = {
    "join-then-stale-second-join": 'no evidence folder',
    "editor-text-mirror": 'no evidence folder',
    "bad-render-quick-open": 'no evidence folder',
    "iroh-proptest-fork-child-wrapper": 'no evidence folder',
    "iroh-proptest-fork-parent-wrapper": 'no evidence folder',
    "deleted-page-rewritten-from-stale-page-mirror-pin": 'no evidence folder',
    "cmd-click-open-tab-integration-state-write": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "hand-authored-renamed-recipe-refusal-not-disclosed-load": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "snapshot-boot-restore-differs-from-first-restore": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "crash-restart-row-vouches-unstamped-base": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "crash-restore-window-not-disclosed": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "iroh-proptest-fork-parent-aborted-before-first-case-load": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "watch-ui-newborn-first-render-missing-row-load": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "deleted-file-recreated-by-stale-page-mirror": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "writeback-undone-deletion-prune-deadline-load-flake": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "writeback-churning-file-stall-disclosed-twice": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "two-instance-viewmodel-stale-displayed-text": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
    "split-doc-root-setup-org-sync-under-load": 'its evidence is a hand-authored case or an excerpt the classifier reads no panic location from',
}
# Rows that also pin one fixed target location, besides MOVED.
STATIC = {
    "undo-prod-session-did-not-revert": "crates/holon-integration-tests/tests/editing_suite/undo_prod_session_wiring.rs:201:9",
}


def run(cmd):
    r = subprocess.run(cmd, capture_output=True, text=True)
    assert r.returncode in (0, 1), f"{cmd} exit {r.returncode}:\n{r.stdout}{r.stderr}"
    return r.returncode, r.stdout


def evidence_logs(repo, key):
    for path in sorted((repo / "docs/Testing/known-red-evidence" / key).glob("*.log*")):
        if path.suffix == ".zst":
            yield path, subprocess.run(["zstd", "-dc", str(path)], check=True, capture_output=True, text=True).stdout
        else:
            yield path, path.read_text()


def attributed_location(classifier, work, key, text):
    """The location the classifier prints for a hit on `key` in `text`, or None."""
    log = work / "evidence.log"
    log.write_text(text)
    out = subprocess.run([str(classifier), str(log)], capture_output=True, text=True).stdout
    found = re.search(rf"^(?:PRIMARY: )?\[known-red:{re.escape(key)}\] \S+ @ (\S+?:\d+:\d+): ", out, re.M)
    return found.group(1) if found else None


def check_row(repo, key, moved):
    """(problem or None, "ok"/"exempt" note) for row `key` with its panic moved to `moved`."""
    classifier = repo / "scripts/keystone-known-reds.sh"
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for path, text in evidence_logs(repo, key):
            loc = attributed_location(classifier, work, key, text)
            if loc is None or f"panicked at {loc}:" not in text:
                continue
            log = work / "moved.log"
            log.write_text(PANIC_HEAD.sub(f"panicked at {moved}:", text))
            code, out = run([str(classifier), str(log)])
            cross = f"cross-family match: {key} at {moved}" in out
            absorbed = f"[known-red:{key}]" not in out and "[known-red:" in out
            if not (cross and code == 1) and not absorbed:
                return f"{key}: {path.name} with every panic moved to {moved}: want `cross-family match: {key}` (exit 1) or another row's hit, got exit {code}:\n{out}"
            return None
        return f"{key}: no evidence log places the row's panic at a location it states"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=str(Path(__file__).resolve().parent.parent))
    ap.add_argument("--anchored-in", help="registry that names the anchored rows (default: the classified tree's own)")
    ap.add_argument("--only", nargs="*", default=[])
    args = ap.parse_args()
    repo = Path(args.repo)
    rows = subprocess.run(["python3", str(repo / "scripts/check-known-red-evidence.py"), "--rows", args.anchored_in or str(repo / "docs/Testing/KeystoneKnownReds.md")],
                          check=True, capture_output=True, text=True).stdout
    anchored = [f[0] for f in (line.split("\x1f") for line in rows.splitlines()) if f[0] and f[2] and (not args.only or f[0] in args.only)]
    assert not set(EXEMPT) - set(anchored) or args.only, f"EXEMPT names rows without an anchor: {sorted(set(EXEMPT) - set(anchored))}"
    jobs = [(key, MOVED) for key in anchored] + [(key, STATIC[key]) for key in anchored if key in STATIC]
    with ThreadPoolExecutor(max_workers=8) as pool:
        problems = list(pool.map(lambda j: check_row(repo, *j), jobs))
    failures, exempt = [], []
    for (key, moved), problem in zip(jobs, problems):
        if problem is None:
            if key in EXEMPT and moved == MOVED:
                failures.append(f"{key}: on EXEMPT but its evidence is covered; remove it from EXEMPT")
            continue
        if key in EXEMPT and moved == MOVED:
            exempt.append(f"{key}: {EXEMPT[key]}")
        else:
            failures.append(problem)
    print(f"[site-anchors] {len(anchored)} anchored row(s), {len(jobs)} case(s), {len(exempt)} exempt")
    for line in exempt:
        print(f"[site-anchors] exempt {line}")
    for line in failures:
        print(f"[site-anchors] FAIL {line}")
    print(f"[site-anchors] {'FAIL' if failures else 'PASS'}: {len(failures)} failing case(s)")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
