#!/usr/bin/env python3
"""Run the known-reds classifier's random-draw drivers in parallel, with one seed.

Run: `python3 scripts/known-reds-property-checks.py` (SEED=<s> replays a draw, CLASSIFIER=<script> names the
classifier under test). The first line names the seed and the draw count of the classifier driver; every driver gets
that seed. Exit 1 when any driver fails, after printing its output.
"""
import os
import random
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
CLASSIFIER_CASES = 5
SIBLING_CASES = 20
SIBLINGS = ["name-boundary", "late-child-result", "spliced-verdict", "trailing-verdict-word", "unseen-verdict", "verdict-word-prefix"]
DRIVERS = [("classifier", {"CASES": str(CLASSIFIER_CASES)})] + [(s, {"CASES": str(SIBLING_CASES)}) for s in SIBLINGS] + [("green-callers", {})]


def run(driver, extra, seed):
    start = time.monotonic()
    r = subprocess.run(["python3", str(SCRIPTS / f"test-known-reds-{driver}.py")], env={**os.environ, "SEED": str(seed), **extra}, capture_output=True, text=True)
    return driver, r.returncode, time.monotonic() - start, r.stdout + r.stderr


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    print(f"drawn with seed {seed} ({CLASSIFIER_CASES} classifier cases, {SIBLING_CASES} per sibling), SEED={seed} replays", flush=True)
    with ThreadPoolExecutor(max_workers=len(DRIVERS)) as pool:
        results = list(pool.map(lambda d: run(*d, seed), DRIVERS))
    for driver, code, secs, out in results:
        print(f"{'PASS' if code == 0 else 'FAIL'} {driver} ({secs:.0f}s)")
        if code != 0:
            print(out)
    sys.exit(1 if any(code for _, code, _, _ in results) else 0)


if __name__ == "__main__":
    main()
