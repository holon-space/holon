"""Property: a failing test's panic stays classified when its `FAILED` stands
behind another thread's short text on its head line (`... 6: FAILED`) and the
next line is another thread's text that starts with a verdict word but is not
one (`okay, retrying`), while another such line (`FAILED to bind ...`) comes
before a passing test's pushed `ok` in the same section.

Run: `python3 scripts/test-known-reds-verdict-word-prefix.py` (CASES=<n>, SEED=<s>).
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
SHORT_TEXT = ["6: ", "5/6: ", "[x] ", "step 3 "]
OK_PREFIXED = ["okay, retrying the handshake", "ok: ()", "ok_count=3", "ok, 2 rows"]
FAILED_PREFIXED = ["FAILED to bind 127.0.0.1:0, retrying", "FAILED: 0 of 3 replicas", "FAILED_ATTEMPTS=1"]


def draw_section(rng):
    """(log text, novel messages) of one cargo section with one failing test."""
    k = rng.randint(1, 3)
    passing = [f"refusals::refuses_{logs.word(rng, set())}_{i}" for i in range(k)]
    failing = "refusals::real_bug"
    body = [f"thread '{failing}' (99) panicked at crates/x/src/bug.rs:1:5:", "REAL PRODUCT BUG"]
    verdicts = [[f"test {failing} ... {rng.choice(SHORT_TEXT)}FAILED", rng.choice(OK_PREFIXED)]]
    for i, name in enumerate(passing):
        verdicts.append([f"test {name} ... ", rng.choice(FAILED_PREFIXED), "ok"] if i == 0 else [f"test {name} ... ok"])
    rng.shuffle(verdicts)
    result = f"test result: FAILED. {k} passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s"
    lines = ["     Running tests/main.rs (target/debug/deps/x-0123)", "", f"running {k + 1} tests", *body]
    for v in verdicts:
        lines += v
    return "\n".join([*lines, "", result]) + "\n", {"REAL PRODUCT BUG"}


def check(work, text, novel, label):
    log = work / "prefix.log"
    log.write_text(text)
    r = subprocess.run([str(CLASSIFIER), str(log)], check=False, capture_output=True, text=True)
    assert r.returncode in (0, 1), f"classifier exit {r.returncode}:\n{r.stdout}{r.stderr}"
    v = logs.parse(r.stdout)
    if not novel <= set(v.listed) or "refusals::real_bug" in v.dropped:
        return [f"{label}: novel {sorted(v.listed)} dropped {sorted(v.dropped)}; want novel {sorted(novel)}, real_bug kept\n--- log\n{text}"]
    return []


EXAMPLE = """     Running tests/main.rs (target/debug/deps/x-0123)

running 2 tests

thread 'refusals::real_bug' (99) panicked at crates/x/src/bug.rs:1:5:
REAL PRODUCT BUG
test refusals::real_bug ... 6: FAILED
okay, retrying the handshake
test refusals::plain_pass ... 
FAILED to bind 127.0.0.1:0, retrying
ok

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
"""


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**31)))
    cases = int(os.environ.get("CASES", "100"))
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        failures += check(work, EXAMPLE, {"REAL PRODUCT BUG"}, "example verdict-word-prefix")
        rng = random.Random(seed)
        for c in range(cases):
            text, novel = draw_section(rng)
            failures += check(work, text, novel, f"case {c}")
    for f in failures[:3]:
        print(f)
    print(f"[verdict-word-prefix] {'FAIL' if failures else 'PASS'}: {len(failures)} failing case(s) of {cases + 1}, seed {seed}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
