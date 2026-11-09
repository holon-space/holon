"""Property: a passing test's panic is dropped when its `ok` follows another
thread's text on the verdict's own line (`test X - should panic ... 6: ok`).

Run: `python3 scripts/test-known-reds-spliced-verdict.py` (CASES=<n>, SEED=<s>).
The fixed example, from a real hand-authored shard, runs first.
"""
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

import known_reds_logs as logs

CLASSIFIER = Path(os.environ.get("CLASSIFIER", Path(__file__).resolve().parent / "keystone-known-reds.sh"))
FOREIGN = ["6: ", "Applying transition 5/6: ", "[inv-sql-budget] not judged "]


def draw_section(rng):
    """(log text, names of the passing tests, novel messages) of one cargo section."""
    k = rng.randint(1, 4)
    names = [f"refusals::refuses_{logs.word(rng, set())}_{i}" for i in range(k)]
    failing = rng.random() < 0.5
    body, novel = [], set()
    for i, name in enumerate(names):
        body += [f"thread '{name}' ({100 + i}) panicked at crates/x/src/refusal.rs:{10 + i}:5:", f"refusal {i} raised"]
    if failing:
        body += ["thread 'refusals::real_bug' (99) panicked at crates/x/src/bug.rs:1:5:", "REAL PRODUCT BUG"]
        novel.add("REAL PRODUCT BUG")
    verdicts = [f"test {n} - should panic ... {rng.choice(FOREIGN)}ok" for n in names]
    if failing:
        verdicts.append("test refusals::real_bug ... FAILED")
    rng.shuffle(verdicts)
    result = f"test result: {'FAILED' if failing else 'ok'}. {k} passed; {int(failing)} failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s"
    text = "\n".join(["     Running tests/main.rs (target/debug/deps/x-0123)", "", f"running {k + failing} tests", *body, *verdicts, "", *(["failures:", "    refusals::real_bug", ""] if failing else []), result]) + "\n"
    return text, set(names), novel


def check(work, text, names, novel, label):
    log = work / "spliced.log"
    log.write_text(text)
    r = subprocess.run([str(CLASSIFIER), str(log)], check=False, capture_output=True, text=True)
    assert r.returncode in (0, 1), f"classifier exit {r.returncode}:\n{r.stdout}{r.stderr}"
    v = logs.parse(r.stdout)
    if set(v.listed) != novel or v.dropped != names:
        return [f"{label}: novel {sorted(v.listed)} dropped {sorted(v.dropped)}; want novel {sorted(novel)} dropped {sorted(names)}\n--- log\n{text}"]
    return []


EVIDENCE = """     Running tests/main.rs (target/debug/build/holon-integration-tests/bf76ca5e48b900b1/out/integration-bf76ca5e48b900b1)

running 2 tests

thread 'hand_authored_regressions::also_check_after_the_last_transition_is_refused' (1934308419) panicked at crates/holon-integration-tests/src/pbt/hand_authored.rs:226:13:
hand-authored regression "<inline>":1 case "n": `also_check_after` names transition 4, out of range 1..3 (1-based; the last transition 4 is always checked)
/4: ()
Applying transition 5/test hand_authored_regressions::also_check_after_the_last_transition_is_refused - should panic ... 6: ok
()
test hand_authored_regressions::judged_windows_start_swept_and_only_pre_sweeps_do_not ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 628 filtered out; finished in 206.82s
"""


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**31)))
    cases = int(os.environ.get("CASES", "100"))
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        failures += check(work, EVIDENCE, {"hand_authored_regressions::also_check_after_the_last_transition_is_refused"}, set(), "example spliced-verdict-evidence")
        rng = random.Random(seed)
        for c in range(cases):
            text, names, novel = draw_section(rng)
            failures += check(work, text, names, novel, f"case {c}")
    for f in failures[:3]:
        print(f)
    print(f"[spliced-verdict] {'FAIL' if failures else 'PASS'}: {len(failures)} failing case(s) of {cases + 1}, seed {seed}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
