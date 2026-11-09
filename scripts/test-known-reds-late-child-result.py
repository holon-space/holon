"""Property: another process's `test result:` line behind its own prefix,
written after the run's real `test result:` line, does not decide the run's
failed count: the passing tests' panics are dropped and the failing test's
panic stays classified.

Run: `python3 scripts/test-known-reds-late-child-result.py` (CASES=<n>, SEED=<s>).
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
PREFIXES = ["[child] ", "[worker 2] ", "INFO spawned: ", "x"]


def late_result(rng, failed):
    """A foreign `test result:` line behind a prefix, with a failed count other than `failed`."""
    other = rng.choice([n for n in range(4) if n != failed])
    word = "FAILED" if other else "ok"
    return f"{rng.choice(PREFIXES)}test result: {word}. {rng.randint(0, 5)} passed; {other} failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s"


def draw_section(rng):
    """(log text, names of passing tests with a panic, novel messages) of one cargo section."""
    k = rng.randint(1, 3)
    failing = rng.random() < 0.5
    passing = [f"refusals::refuses_{logs.word(rng, set())}_{i}" for i in range(k)]
    body, heads = [], []
    for i, name in enumerate(passing):
        body += [f"thread '{name}' ({100 + i}) panicked at crates/x/src/refusal.rs:{10 + i}:5:", f"refusal {i} raised"]
        heads.append(f"test {name} - should panic ... ok")
    if failing:
        body += ["thread 'refusals::real_bug' (99) panicked at crates/x/src/bug.rs:1:5:", "REAL PRODUCT BUG"]
        heads.append("test refusals::real_bug ... FAILED")
    rng.shuffle(heads)
    word = "FAILED" if failing else "ok"
    result = f"test result: {word}. {k} passed; {int(failing)} failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s"
    listed = ["failures:", "    refusals::real_bug", ""] if failing else []
    late = [late_result(rng, int(failing)) for _ in range(rng.randint(1, 2))]
    text = "\n".join(["     Running tests/main.rs (target/debug/deps/x-0123)", "", f"running {k + failing} tests", *body, *heads, "", *listed, result, *late, ""]) + "\n"
    return text, set(passing), {"REAL PRODUCT BUG"} if failing else set()


def check(work, text, names, novel, label):
    log = work / "late.log"
    log.write_text(text)
    r = subprocess.run([str(CLASSIFIER), str(log)], check=False, capture_output=True, text=True)
    assert r.returncode in (0, 1), f"classifier exit {r.returncode}:\n{r.stdout}{r.stderr}"
    v = logs.parse(r.stdout)
    if set(v.listed) != novel or v.dropped != names:
        return [f"{label}: novel {sorted(v.listed)} dropped {sorted(v.dropped)}; want novel {sorted(novel)} dropped {sorted(names)}\n--- log\n{text}"]
    return []


EXAMPLE = """     Running tests/main.rs (target/debug/deps/x-0123)

running 2 tests
thread 'refusals::refuses_a' (100) panicked at crates/x/src/refusal.rs:10:5:
refusal 0 raised
test refusals::refuses_a - should panic ... ok
test refusals::plain_pass ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
[child] test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
"""


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**31)))
    cases = int(os.environ.get("CASES", "100"))
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        failures += check(work, EXAMPLE, {"refusals::refuses_a"}, set(), "example late-child-result")
        rng = random.Random(seed)
        for c in range(cases):
            text, names, novel = draw_section(rng)
            failures += check(work, text, names, novel, f"case {c}")
    for f in failures[:2]:
        print(f)
    print(f"[late-child-result] {'FAIL' if failures else 'PASS'}: {len(failures)} failing case(s) of {cases + 1}, seed {seed}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
