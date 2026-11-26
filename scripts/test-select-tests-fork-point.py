#!/usr/bin/env python3
"""Property check that scripts/select-tests.py diffs a lane from its fork point, not from the moving base.

Run from anywhere: `python3 scripts/test-select-tests-fork-point.py` (SEED=<n>, CASES=<n>, default 40).
Each case builds a scratch jj repo holding the selector's fixture workspace, forks a lane (one or two changes, the
second being a fix round) off `main`, and then lands drawn files on `main`. The selection of the lane must not move:
- `fork_point("main")` is the commit `main` had at the fork, and `BASE:` names it,
- `jj_changed` lists exactly the lane's own files (a rename-free diff, so the files drawn),
- the step lines equal the lines selected before the landing,
- `--changed-paths main`, which `just lane-checks` reads, prints the same changed paths.
Fixed cases first: the shape of a lane whose base moved under it, and a base that does not resolve.
Exits non-zero when a property fails or a named input class was never drawn.
"""
import contextlib
import importlib.util
import io
import os
import random
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("select_tests", REPO / "scripts/select-tests.py")
st = importlib.util.module_from_spec(spec)
spec.loader.exec_module(st)

POOL = ["crates/core/src/lib.rs", "crates/core/src/a.rs", "crates/rows/src/lib.rs", "crates/app/src/lib.rs",
        "crates/leaf/src/lib.rs", "crates/bare/src/lib.rs", "crates/ofmt/src/lib.rs", "assets/default/seed.yaml",
        "docs/x.md", "docs/y.md"]
EXAMPLES = [
    # The lane forked off an older main; two landings later `main` holds unrelated crates and docs.
    {"lane": ["crates/core/src/lib.rs"], "fix": [], "landed": ["crates/leaf/src/lib.rs", "docs/x.md", "crates/rows/src/lib.rs"]},
    {"lane": ["crates/core/src/lib.rs", "docs/y.md"], "fix": ["docs/y.md"], "landed": ["crates/app/src/lib.rs"]},
]


def jj(root, *args):
    done = subprocess.run(["jj", *args], cwd=root, capture_output=True, text=True)
    assert done.returncode == 0, f"jj {' '.join(args)}: {done.stderr}"
    return done.stdout


def commit_id(root, rev):
    return jj(root, "log", "--no-graph", "-r", rev, "-T", "commit_id").strip()


def selected(root, meta, changed):
    with contextlib.redirect_stderr(io.StringIO()):
        return st.selection(root, meta, changed)


def write(root, paths, tag):
    for p in paths:
        (root / p).parent.mkdir(parents=True, exist_ok=True)
        (root / p).write_text(f"{tag} {p}\n")


def run_case(case, counts):
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp).resolve()
        jj(root, "git", "init", ".")
        meta = st.fixture_workspace(root)
        jj(root, "commit", "-m", "base")
        jj(root, "bookmark", "create", "main", "-r", "@-")
        old_main = commit_id(root, "main")
        jj(root, "new", "main", "-m", "lane")
        write(root, case["lane"], "lane")
        if case["fix"]:
            jj(root, "new", "-m", "fix round")
            write(root, case["fix"], "fix")
        lane_tip = commit_id(root, "@")
        os.chdir(root)
        before = selected(root, meta, st.jj_changed(st.fork_point("main")))
        jj(root, "new", "main", "-m", "landed")
        write(root, case["landed"], "landed")
        jj(root, "bookmark", "set", "main", "-r", "@")
        jj(root, "edit", lane_tip)
        fork = st.fork_point("main")
        changed = st.jj_changed(fork)
        after = selected(root, meta, changed)
        header = st.jj_header(fork)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            assert st.main(["--changed-paths", "main"]) == 0
        found = []
        if fork != old_main:
            found.append(f"fork point {fork} is not the old main {old_main}")
        if header[0] != f"BASE: {old_main}":
            found.append(f"header {header[0]!r} does not name the old main {old_main}")
        want = sorted(set(case["lane"]) | set(case["fix"]))
        if changed != want:
            found.append(f"changed paths {changed} are not the lane's own {want}; landed {case['landed']}")
        if out.getvalue().splitlines() != want:
            found.append(f"--changed-paths printed {out.getvalue().splitlines()}, not the lane's own {want}")
        if after != before:
            found.append(f"step lines moved with the landing:\n  before {before}\n  after  {after}")
        counts["cases"] += 1
        counts["fix round"] += bool(case["fix"])
        counts["landed disjoint from lane"] += bool(set(case["landed"]) - set(want))
        counts["landed overlaps lane"] += bool(set(case["landed"]) & set(want))
        return found


def unknown_base_problems():
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp).resolve()
        jj(root, "git", "init", ".")
        os.chdir(root)
        try:
            st.fork_point("no-such-revision-zz")
        except SystemExit as e:
            return [] if "fork_point(@ | no-such-revision-zz)" in str(e.code) else [f"message lacks the revset: {e.code}"]
        return ["an unknown base did not exit"]


def draw(rng):
    lane = rng.sample(POOL, rng.randint(1, 4))
    fix = rng.sample(lane, rng.randint(0, len(lane))) if rng.random() < 0.4 else []
    return {"lane": lane, "fix": fix, "landed": rng.sample(POOL, rng.randint(1, 5))}


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**32)))
    rng = random.Random(seed)
    counts, failures = Counter(), 0
    cases = EXAMPLES + [draw(rng) for _ in range(int(os.environ.get("CASES", "40")))]
    for case in cases:
        for f in run_case(case, counts):
            print(f"FAIL: {case}: {f}")
            failures += 1
    for f in unknown_base_problems():
        print(f"FAIL: unknown base: {f}")
        failures += 1
    for cls in ("fix round", "landed disjoint from lane", "landed overlaps lane"):
        if not counts[cls]:
            print(f"FAIL: input class never drawn: {cls}")
            failures += 1
    print(f"seed {seed}: {dict(counts)}; {'FAIL' if failures else 'PASS'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
