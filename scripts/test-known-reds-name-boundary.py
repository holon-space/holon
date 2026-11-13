#!/usr/bin/env python3
"""Property check of scripts/keystone-known-reds.sh: a split panic head of a thread
whose name only starts with a passing test's name (a name character follows it)
is not that test's, so the classifier judges it as it judges the same head closed.

Run: `python3 scripts/test-known-reds-name-boundary.py` (CASES=<n>, SEED=<s>).
"""
import os
import random
import string
import subprocess
import sys
import tempfile
from pathlib import Path

CLASSIFIER = Path(os.environ.get("CLASSIFIER", Path(__file__).resolve().parent / "keystone-known-reds.sh"))
NAME_CHARS = string.ascii_lowercase + string.digits + "_:-"
FIXED = [("refusals::refuses", "-worker"), ("pool", "_2")]


def log(test, thread, split):
    head = [f"thread '{thread}", "' (5) panicked at crates/x.rs:1:1:"] if split else [f"thread '{thread}' (5) panicked at crates/x.rs:1:1:"]
    return "\n".join(["     Running tests/main.rs (target/debug/deps/x-0123)", "", "running 1 test", *head, "worker died",
                      f"test {test} ... ok", "", "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s", ""])


def classify(work, text):
    path = work / "case.log"
    path.write_text(text)
    r = subprocess.run([str(CLASSIFIER), str(path)], capture_output=True, text=True,
                       env=dict(os.environ, LC_ALL="en_US.UTF-8", TMPDIR=str(work)))
    return r.returncode, "worker died" in r.stdout


def main():
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    rng = random.Random(seed)
    cases = FIXED + [("::".join("".join(rng.choice(string.ascii_lowercase) for _ in range(rng.randint(2, 8))) for _ in range(rng.randint(1, 3))),
                      rng.choice("_:-0123456789") + "".join(rng.choice(NAME_CHARS) for _ in range(rng.randint(0, 6))))
                     for _ in range(int(os.environ.get("CASES", 20)))]
    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for test, tail in cases:
            closed, split = classify(work, log(test, test + tail, False)), classify(work, log(test, test + tail, True))
            if closed != split:
                failed += 1
                print(f"test {test!r}, thread {test + tail!r}: closed head (exit, printed) {closed}, split head {split}")
    print(f"[name-boundary] {'FAIL' if failed else 'PASS'}: {failed} of {len(cases)} case(s), seed {seed}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
