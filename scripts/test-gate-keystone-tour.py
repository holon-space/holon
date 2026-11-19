#!/usr/bin/env python3
"""Property check of the keystone-tour step of scripts/gate-integrate-steps.sh against the model of scripts/known_reds_logs.py.

Run: `python3 scripts/test-gate-keystone-tour.py` (CASES=<n> draws n logs, default 40; SEED=<s> replays a draw). A stub
`just` prints a drawn log and exits 101, as a red `cargo test` does. Per draw, with and without a rerun's GATE_ONLY_IDS:
- the model's verdict is known reds only (no novel panic, one known at least): the step ends 0, prints the note
  and records no failed ids;
- otherwise the step keeps the tour's exit code (the recorder's exit 2 when it cannot name the ids) and records the
  ids `gate-failed-ids.py` names for the log;
- either way the classifier's output is in target/gate-logs/<gate>-tour-classify.txt.
Fixed cases run first: a red tour that printed no panic, and one novel panic among known ones. A green tour, whatever
log it printed, ends 0 with no classification.
"""
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

import known_reds_logs as logs

REPO = Path(__file__).resolve().parent.parent
GATE = "g"
NOTE = "keystone-tour: pass-with-note (known reds only)"
STUB = '#!/bin/sh\nfor a; do [ "$a" != --evaluate ] || { echo stub; exit 0; }; done\ncat "$STUB_LOG"\nexit "$STUB_EXIT"\n'
RERUN_ID = "holon-integration-tests::integration keystone_tour::a::case_b"


def tree(root):
    (root / "docs/Testing").mkdir(parents=True)
    (root / "target/gate-logs").mkdir(parents=True)
    (root / "bin").mkdir()
    (root / "scripts").symlink_to(REPO / "scripts")
    (root / "justfile").symlink_to(REPO / "justfile")
    (root / "docs/Testing/known-red-evidence").symlink_to(REPO / "docs/Testing/known-red-evidence")
    (root / "docs/Testing/KeystoneKnownReds.md").symlink_to(REPO / "docs/Testing/KeystoneKnownReds.md")
    (root / "bin/just").write_text(STUB)
    (root / "bin/just").chmod(0o755)


def step(root, text, tour_exit, rerun):
    (root / "tour.log").write_text(text)
    ids = root / "ids"
    ids.unlink(missing_ok=True)
    env = {**os.environ, "PATH": f"{root / 'bin'}:{os.environ['PATH']}", "STUB_LOG": str(root / "tour.log"), "STUB_EXIT": str(tour_exit),
           "GATE": GATE, "PBT_FEATURES": "stub", "GATE_IDS_FILE": str(ids)}
    env.pop("GATE_ONLY_IDS", None)
    if rerun:
        env["GATE_ONLY_IDS"] = RERUN_ID
    r = subprocess.run(["bash", "scripts/gate-integrate-steps.sh", "keystone-tour"], cwd=root, env=env, capture_output=True, text=True)
    return r, ids.read_text() if ids.exists() else ""


def recorded(root, log):
    """(exit, failed-id lines) of the recorder on the tour log the step wrote."""
    r = subprocess.run(["python3", str(REPO / "scripts/gate-failed-ids.py"), "keystone-tour", str(log)], capture_output=True, text=True)
    return r.returncode, "".join(f"failed {i}\n" for i in r.stdout.split("\n") if i)


def failures(root, name, text, known_only, rerun):
    r, ids = step(root, text, 101, rerun)
    log = root / f"target/gate-logs/{GATE}-tour.log"
    classified = root / f"target/gate-logs/{GATE}-tour-classify.txt"
    out = f"{name} rerun={rerun}: exit {r.returncode}\n{r.stdout}{r.stderr}\n  ids {ids!r}\n  log:\n{text}"
    if not classified.exists() or "[known-reds]" not in classified.read_text():
        return [f"no classifier output in {classified.name}: {out}"]
    if known_only:
        return [] if (r.returncode, ids) == (0, "") and NOTE in r.stdout.splitlines() else [f"want pass with the note and no ids: {out}"]
    rec_exit, want_ids = recorded(root, log)
    want = (2 if rec_exit else 101, "" if rec_exit else want_ids)
    return [] if (r.returncode, ids) == want and NOTE not in r.stdout else [f"want exit {want[0]}, ids {want[1]!r}, no note: {out}"]


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    cases = int(os.environ.get("CASES", 40))
    rng = random.Random(seed)
    bad = []
    with tempfile.TemporaryDirectory() as tmp:
        n = [0]

        def fresh():
            n[0] += 1
            root = Path(tmp) / f"t{n[0]}"
            tree(root)
            return root

        no_panic = logs.green_run("holon-x::main a::case_b", "holon-x::main a::case_c").replace("PASS", "FAIL", 1).replace("1.000s] 2 tests run: 2 passed", "1.000s] 2 tests run: 1 passed, 1 failed")
        novel_among_known = logs.no_capture_run("thread 'a_controller_dies' (13) panicked at crates/holon-filesystem/src/novel.rs:1:1:", "novel defect zzz", "FAIL")
        known_only = logs.no_capture_run(logs.CRASH_HEAD, logs.CRASH_MSG, "PASS")
        fixed = [("no panic printed", no_panic, False), ("one novel among known", novel_among_known, False), ("known only", known_only, True)]
        for name, text, known in fixed:
            for rerun in (False, True):
                bad += failures(fresh(), name, text, known, rerun)
        for i in range(cases):
            rendered = logs.render(logs.draw(rng), random.Random(rng.randrange(1 << 30)), interleave=i % 2 == 1)
            owed = logs.verdict(rendered)
            bad += failures(fresh(), f"drawn {i}", rendered.text, owed.exit == 0 and owed.matched > 0, i % 3 == 0)
        green_root = fresh()
        for name, text, _ in fixed:
            r, ids = step(green_root, text, 0, False)
            if r.returncode != 0 or ids or (green_root / f"target/gate-logs/{GATE}-tour-classify.txt").exists():
                bad.append(f"green tour printing {name}: exit {r.returncode}, ids {ids!r}\n{r.stdout}{r.stderr}")
    print("\n".join(bad))
    print(f"[gate-keystone-tour] {'FAIL' if bad else 'PASS'}: {len(bad)} failing case(s), seed {seed}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
