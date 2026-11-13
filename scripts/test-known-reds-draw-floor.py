#!/usr/bin/env python3
"""Each draw kind of scripts/known_reds_logs.py that test-known-reds-classifier.py
checks with `reached` is drawn in every run of at least as many cases as there are
new kinds, whatever the seed: the floor holds by construction, not by luck.

Run: `python3 scripts/test-known-reds-draw-floor.py` (RUNS=<n> random seeds, default 3).
"""
import os
import random
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import known_reds_logs as logs  # noqa: E402

KINDS = ["two heads opened on one line", "a chunk between a head and its message",
         "a panic head right after a head", "a verdict line glued behind an open head",
         "a head opened after two heads were open"]
FIXED_SEEDS = [101, 102, 103, 104]


def unreached(seed):
    env = dict(os.environ, SEED=str(seed), CASES=str(len(KINDS)))
    out = subprocess.run([sys.executable, str(HERE / "test-known-reds-classifier.py")], env=env,
                         capture_output=True, text=True).stdout
    counts = dict(re.findall(r"^draws reaching (.*): (\d+)$", out, re.M))
    return [k for k in KINDS if int(counts[k]) == 0]


def main():
    assert all(k in logs.DRAWN for k in KINDS)
    seeds = FIXED_SEEDS + [random.randrange(1 << 30) for _ in range(int(os.environ.get("RUNS", 3)))]
    failed = 0
    for seed in seeds:
        missing = unreached(seed)
        if missing:
            failed += 1
            print(f"seed {seed}, {len(KINDS)} cases: never drawn: {missing}")
    print(f"[draw-floor] {'FAIL' if failed else 'PASS'}: {failed} of {len(seeds)} seed(s) miss a kind")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
