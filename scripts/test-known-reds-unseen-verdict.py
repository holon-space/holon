"""Property: a failing test's panic stays classified when another thread's
line that ends in `ok` follows its `test X ... ` head and the real `FAILED`
is not read in that window: glued to another thread's word (`step6FAILED`),
or behind another thread's `test result:` line that closes the window. The
same shapes with `FAILED` and `ok` swapped stand after a passing test's head
in the same section.

Run: `python3 scripts/test-known-reds-unseen-verdict.py` (CASES=<n>, SEED=<s>).
The fixed example runs first.
"""
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

import known_reds_logs as logs

CLASSIFIER = Path(os.environ.get("CLASSIFIER", Path(__file__).resolve().parent / "keystone-known-reds.sh"))
ENDS_OK = ["[sync] peer handshake: ok", "status=ok"]
ENDS_FAILED = ["[inv-sql-budget] check FAILED", "health FAILED"]
CHILD_RESULT = "[child] test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s"


def hidden(rng, word):
    """Lines that hold the real verdict `word` where the window does not read it."""
    return [f"step{rng.randint(1, 9)}{word}"] if rng.random() < 0.5 else [CHILD_RESULT, word]


def draw_section(rng):
    """(log text, novel messages) of one cargo section with one failing test."""
    k = rng.randint(1, 3)
    passing = [f"refusals::refuses_{logs.word(rng, set())}_{i}" for i in range(k)]
    failing = "refusals::real_bug"
    body = [f"thread '{failing}' (99) panicked at crates/x/src/bug.rs:1:5:", "REAL PRODUCT BUG"]
    verdicts = [[f"test {failing} ... ", rng.choice(ENDS_OK), *hidden(rng, "FAILED")]]
    for i, name in enumerate(passing):
        verdicts.append([f"test {name} ... ", rng.choice(ENDS_FAILED), *hidden(rng, "ok")] if i == 0 else [f"test {name} ... ok"])
    rng.shuffle(verdicts)
    result = f"test result: FAILED. {k} passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s"
    lines = ["     Running tests/main.rs (target/debug/deps/x-0123)", "", f"running {k + 1} tests", *body]
    for v in verdicts:
        lines += v
    return "\n".join([*lines, "", result]) + "\n", {"REAL PRODUCT BUG"}


def check(work, text, novel, label):
    log = work / "unseen.log"
    log.write_text(text)
    r = subprocess.run([str(CLASSIFIER), str(log)], check=False, capture_output=True, text=True)
    assert r.returncode in (0, 1), f"classifier exit {r.returncode}:\n{r.stdout}{r.stderr}"
    v = logs.parse(r.stdout)
    if not novel <= set(v.listed) or "refusals::real_bug" in v.dropped:
        return [f"{label}: novel {sorted(v.listed)} dropped {sorted(v.dropped)}; want novel {sorted(novel)}, real_bug kept\n--- log\n{text}"]
    return []


EXAMPLE = f"""     Running tests/main.rs (target/debug/deps/x-0123)

running 2 tests

thread 'refusals::real_bug' (99) panicked at crates/x/src/bug.rs:1:5:
REAL PRODUCT BUG
test refusals::real_bug ... 
[sync] peer handshake: ok
{CHILD_RESULT}
FAILED
test refusals::plain_pass ... 
[inv-sql-budget] check FAILED
step6ok

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
"""


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**31)))
    cases = int(os.environ.get("CASES", "100"))
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        failures += check(work, EXAMPLE, {"REAL PRODUCT BUG"}, "example unseen-verdict")
        rng = random.Random(seed)
        for c in range(cases):
            text, novel = draw_section(rng)
            failures += check(work, text, novel, f"case {c}")
    for f in failures[:2]:
        print(f)
    print(f"[unseen-verdict] {'FAIL' if failures else 'PASS'}: {len(failures)} failing case(s) of {cases + 1}, seed {seed}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
