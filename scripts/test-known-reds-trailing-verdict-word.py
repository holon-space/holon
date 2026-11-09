"""Property: a failing test's panic stays classified when another thread's text
that ends in a verdict word (`... status: ok`) lands after its `test X ... `
head, and another such text (`... check FAILED`) lands after a passing test's
head in the same section.

Run: `python3 scripts/test-known-reds-trailing-verdict-word.py` (CASES=<n>, SEED=<s>).
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
TRAILING_TEXT = ["[sync] peer handshake: ", "[inv-sql-budget] check ", "status=", "health "]


def draw_section(rng):
    """(log text, names of the passing tests with a panic, novel messages) of one cargo section."""
    k = rng.randint(1, 3)
    passing = [f"refusals::refuses_{logs.word(rng, set())}_{i}" for i in range(k)]
    failing = "refusals::real_bug"
    body = []
    for i, name in enumerate(passing):
        body += [f"thread '{name}' ({100 + i}) panicked at crates/x/src/refusal.rs:{10 + i}:5:", f"refusal {i} raised"]
    body += [f"thread '{failing}' (99) panicked at crates/x/src/bug.rs:1:5:", "REAL PRODUCT BUG"]
    verdicts = [[f"test {failing} ... {rng.choice(TRAILING_TEXT)}ok", "FAILED"]]
    for i, name in enumerate(passing):
        tail = f"{rng.choice(TRAILING_TEXT)}FAILED" if i == 0 else ""
        verdicts.append([f"test {name} - should panic ... {tail}", "ok"] if tail else [f"test {name} - should panic ... ok"])
    rng.shuffle(verdicts)
    result = f"test result: FAILED. {k} passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s"
    lines = ["     Running tests/main.rs (target/debug/deps/x-0123)", "", f"running {k + 1} tests", *body]
    for v in verdicts:
        lines += v
    text = "\n".join([*lines, "", "failures:", f"    {failing}", "", result]) + "\n"
    return text, set(passing), {"REAL PRODUCT BUG"}


def check(work, text, names, novel, label):
    log = work / "trailing.log"
    log.write_text(text)
    r = subprocess.run([str(CLASSIFIER), str(log)], check=False, capture_output=True, text=True)
    assert r.returncode in (0, 1), f"classifier exit {r.returncode}:\n{r.stdout}{r.stderr}"
    v = logs.parse(r.stdout)
    if set(v.listed) != novel or v.dropped != names:
        return [f"{label}: novel {sorted(v.listed)} dropped {sorted(v.dropped)}; want novel {sorted(novel)} dropped {sorted(names)}\n--- log\n{text}"]
    return []


EXAMPLE = """     Running tests/main.rs (target/debug/deps/x-0123)

running 2 tests

thread 'refusals::real_bug' (99) panicked at crates/x/src/bug.rs:1:5:
REAL PRODUCT BUG
test refusals::real_bug ... [sync] peer handshake: ok
FAILED
test refusals::refuses_a - should panic ... [inv-sql-budget] check FAILED
ok

failures:
    refusals::real_bug

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
"""


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**31)))
    cases = int(os.environ.get("CASES", "100"))
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        failures += check(work, EXAMPLE, set(), {"REAL PRODUCT BUG"}, "example trailing-verdict-words")
        rng = random.Random(seed)
        for c in range(cases):
            text, names, novel = draw_section(rng)
            failures += check(work, text, names, novel, f"case {c}")
    for f in failures[:3]:
        print(f)
    print(f"[trailing-verdict-word] {'FAIL' if failures else 'PASS'}: {len(failures)} failing case(s) of {cases + 1}, seed {seed}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
