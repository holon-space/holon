#!/usr/bin/env python3
"""Property check: test-cargo-free-checks.py's `copy_tree` copies the files of the tree it runs in.

Run: `python3 scripts/test-copy-tree-tracks-tree-under-test.py` (CASES=<n> draws n cases, default 10).
A tester copy holds the implementer's commit as recorded plus the tester's new files, while the working copy of
`LANE_REPO` moves on. Each case draws files the working copy gained after the copy and files only the copy holds;
the copied tree must hold exactly the files of the tree under test. EXAMPLES run first.
"""
import importlib.util
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("cargo_free_checks", HERE / "test-cargo-free-checks.py")
cfc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cfc)

# (files the LANE_REPO working copy gained after the copy, files only the tester copy holds)
EXAMPLES = [(["scripts/late.py"], []), ([], ["scripts/test-new.py"])]
NAMES = ["scripts/a.py", "scripts/b.sh", "docs/x.md", "crates/c/src/lib.rs", "justfile"]


def draw(rng):
    pool = [f"scripts/n{i}.py" for i in range(6)]
    rng.shuffle(pool)
    k, m = rng.randrange(3), rng.randrange(3)
    return pool[:k], pool[k:k + m]


def files(root):
    return {str(p.relative_to(root)) for p in root.rglob("*") if p.is_file() and ".git" not in p.parts and ".jj" not in p.parts}


def write(root, rels):
    for rel in rels:
        (root / rel).parent.mkdir(parents=True, exist_ok=True)
        (root / rel).write_text(rel)


def check(case):
    gained, tester_only = case
    with tempfile.TemporaryDirectory() as d:
        d = Path(d)
        lane, copy, dest = d / "lane", d / "copy", d / "dest"
        write(lane, NAMES)
        subprocess.run(["jj", "git", "init", "--quiet"], cwd=lane, check=True)
        write(copy, NAMES + tester_only)
        write(lane, gained)
        subprocess.run(["jj", "-R", str(lane), "status", "--quiet"], check=True, stdout=subprocess.DEVNULL)
        os.environ["LANE_REPO"] = str(lane)
        cfc.REPO = copy
        try:
            cfc.copy_tree(dest)
        except Exception as e:
            return f"copy_tree raised {type(e).__name__}: {e}"
        if files(dest) != files(copy):
            return f"copied {sorted(files(dest))}, tree under test {sorted(files(copy))}"
    return None


def main():
    seed = random.randrange(2**32)
    rng = random.Random(seed)
    cases = EXAMPLES + [draw(rng) for _ in range(int(os.environ.get("CASES", "10")))]
    failures = [(case, why) for case in cases if (why := check(case))]
    for case, why in failures:
        print(f"FAIL EXAMPLES entry {case}: {why}")
    print(f"seed {seed}: {len(cases)} cases, {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
