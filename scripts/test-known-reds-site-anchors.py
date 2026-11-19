#!/usr/bin/env python3
"""A dropped site anchor makes scripts/known-reds-site-anchors.py fail, for the undo row and for drawn rows.

Run: `python3 scripts/test-known-reds-site-anchors.py` (CASES=<n> draws n rows besides the undo row, default 3;
SEED=<s> replays a draw). Assures what the loop's own PASS cannot: that it is not vacuous. Each row is checked
with its anchor present (the loop passes) and with that anchor removed from a copy of the registry (the loop fails).
"""
import importlib.util
import os
import random
import re
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
REPO = SCRIPTS.parent
spec = importlib.util.spec_from_file_location("anchors", SCRIPTS / "known-reds-site-anchors.py")
anchors = importlib.util.module_from_spec(spec)
spec.loader.exec_module(anchors)


def covered_keys():
    rows = subprocess.run(["python3", str(SCRIPTS / "check-known-red-evidence.py"), "--rows", str(REPO / "docs/Testing/KeystoneKnownReds.md")],
                          check=True, capture_output=True, text=True).stdout
    return [f[0] for f in (line.split("\x1f") for line in rows.splitlines()) if f[0] and f[2] and f[0] not in anchors.EXEMPT]


def loop(repo, key):
    r = subprocess.run(["python3", str(SCRIPTS / "known-reds-site-anchors.py"), "--repo", str(repo), "--anchored-in", str(REPO / "docs/Testing/KeystoneKnownReds.md"), "--only", key], capture_output=True, text=True)
    assert r.returncode in (0, 1), f"exit {r.returncode}:\n{r.stdout}{r.stderr}"
    return r.returncode, r.stdout


def tree_without_anchor(work, key):
    tree = work / key
    (tree / "docs/Testing").mkdir(parents=True)
    (tree / "scripts").symlink_to(SCRIPTS)
    (tree / "docs/Testing/known-red-evidence").symlink_to(REPO / "docs/Testing/known-red-evidence")
    registry = (REPO / "docs/Testing/KeystoneKnownReds.md").read_text()
    dropped, n = re.subn(rf"^(\| `{re.escape(key)}` \| known-red \| `[^`]*` \| )`[^`]*`( \|)", "\\1\u2014\\2", registry, flags=re.M)
    assert n == 1, f"{key}: want one known-red row with an anchor, found {n}"
    (tree / "docs/Testing/KeystoneKnownReds.md").write_text(dropped)
    return tree


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    rng = random.Random(seed)
    keys = covered_keys()
    drawn = ["undo-prod-session-did-not-revert"] + rng.sample(keys, int(os.environ.get("CASES", 3)))
    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        for key in drawn:
            with_anchor, out = loop(REPO, key)
            if with_anchor != 0:
                failed += 1
                print(f"{key}: the loop fails with the anchor present:\n{out}")
            without, out = loop(tree_without_anchor(Path(tmp), key), key)
            if without != 1:
                failed += 1
                print(f"{key}: the loop passes with the anchor removed:\n{out}")
            else:
                print(f"{key}: red without its anchor, green with it")
    print(f"[site-anchors-mutants] {'FAIL' if failed else 'PASS'}: {failed} failing case(s), seed {seed}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
