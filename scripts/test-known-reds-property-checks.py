#!/usr/bin/env python3
"""Check that only the gating seed of scripts/known-reds-property-checks.py decides its exit code.

Run: `python3 scripts/test-known-reds-property-checks.py` (CASES=<n>, SEED=<s> replays a draw, CHECK=<path> tests
another copy of the script against this tree's GATE_SEED). The script runs from a copy whose drivers are stubs that exit with the code of CLASSIFIER, a stub green only on the seeds in STUB_GREEN.
Properties, fixed examples first: the exit code is 0 exactly when the gating seed (GATE_SEED, or SEED when set) is
green; the first line names that seed and the draw counts; no other seed runs.
"""
import importlib.util
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "known-reds-property-checks.py"
CHECK = Path(os.environ.get("CHECK", SCRIPT))
_spec = importlib.util.spec_from_file_location("property_checks", SCRIPT)
checks = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(checks)
GATE = checks.GATE_SEED
DRIVER_STUB = "import os, subprocess, sys\nsys.exit(subprocess.run([os.environ['CLASSIFIER']]).returncode)\n"
CLASSIFIER_STUB = "#!/bin/sh\ncase \",$STUB_GREEN,\" in *\",$SEED,\"*) exit 0;; esac\nexit 1\n"
# (SEED or None, green seeds): green only on the gate seed; red on the gate seed; a red replay.
EXAMPLES = [(None, {GATE}), (None, set()), (7, {GATE})]


def tree(root):
    (root / "scripts").mkdir()
    shutil.copy(CHECK, root / "scripts" / SCRIPT.name)
    for driver, _ in checks.DRIVERS:
        (root / "scripts" / f"test-known-reds-{driver}.py").write_text(DRIVER_STUB)
    stub = root / "classifier"
    stub.write_text(CLASSIFIER_STUB)
    stub.chmod(0o755)
    return root / "scripts" / SCRIPT.name, stub


def check(script, stub, replay, green):
    env = {k: v for k, v in os.environ.items() if k != "SEED"}
    env.update(CLASSIFIER=str(stub), STUB_GREEN=",".join(map(str, sorted(green))))
    if replay is not None:
        env["SEED"] = str(replay)
    r = subprocess.run([sys.executable, str(script)], env=env, capture_output=True, text=True)
    lines = r.stdout.splitlines()
    gating = GATE if replay is None else replay
    errors = []
    if r.returncode != (0 if gating in green else 1):
        errors.append(f"exit {r.returncode} with gating seed {gating} {'green' if gating in green else 'red'}")
    if not lines or not lines[0].startswith(f"drawn with seed {gating} ({checks.COUNTS})"):
        errors.append(f"first line does not name seed {gating} and the draw counts: {lines[:1]}")
    results = [l for l in lines if re.match(r"(PASS|FAIL) ", l)]
    expect = "PASS" if gating in green else "FAIL"
    if len(results) != len(checks.DRIVERS) or any(not l.startswith(expect) for l in results):
        errors.append(f"not one {expect} line per driver for seed {gating}: {results}")
    return [f"SEED={replay} green={sorted(green)}: {e}\n{r.stdout}{r.stderr}" for e in errors]


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    rng = random.Random(seed)
    drawn = []
    for _ in range(int(os.environ.get("CASES", 10))):
        replay = rng.choice([None, rng.randrange(1 << 30)])
        pool = [GATE, rng.randrange(1 << 30)] + ([] if replay is None else [replay])
        drawn.append((replay, {s for s in pool if rng.random() < 0.5}))
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        script, stub = tree(Path(tmp))
        for replay, green in EXAMPLES + drawn:
            failures += check(script, stub, replay, green)
    for f in failures:
        print(f"[known-reds-property-checks] FAIL: {f}")
    print(f"[known-reds-property-checks] {'FAIL' if failures else 'PASS'}: {len(failures)} failing case(s), seed {seed}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
