#!/usr/bin/env python3
"""Property check for scripts/lane-nextest.py: a real nextest run over a generated fixture crate.

Run: `python3 scripts/test-lane-nextest.py [cases]`. Builds a one-crate workspace in a temp dir (cargo), then exits
non-zero on the first violation.
The crate's tests t00..t07 run one at a time in name order; the env var PLAN gives each test's outcome:
`P` passes, `K` panics with the wording of a known-red row of docs/Testing/KeystoneKnownReds.md, `U` panics with a
wording no row lists.
Generators: the plan (each test P, K or U) and the run filter (a random subset of the tests excluded by `-E`).
Properties, per case: the run completes every filtered test with its planned outcome, exits non-zero iff a `U` ran
and names a novel signature iff it did, its red list names exactly its red tests, and the rerun from that list runs
exactly the listed tests with the same filter.
Fixed plans run first through the same properties: the orders of red kinds a run can end in (a novel red last, a
novel red before a known one, no red) and earlier finds.
A `nextest:` step line of scripts/flow-checks/run-step.sh runs through scripts/lane-nextest.py, and runs on past a
novel red whose later tests take seconds each.
"""
import os
import random
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from nextest_fixture import N, REPO, STATUS, TESTS, UNLISTED, fixture, known_red_wording, ran_tests

SCRIPT = REPO / "scripts" / "lane-nextest.py"
EXAMPLES = [("PPPUPPPP", set()), ("PKPPPKPP", set()), ("PPPPPPPP", set()), ("UPPPPPPP", {"t00"}),
            ("KPKPUPPP", {"t02"}), ("PKKPKUPP", {"t01", "t02"}), ("KKKPPPPP", set()),
            ("PKUUPUPU", {"t07"}), ("PKPKPPPU", set()), ("PPPPPPPU", set()), ("PPUPKPPP", set()),
            ("PPPPPPPP", set(TESTS))]
SUMMARY = re.compile(r"^ *Summary \[.*\] (\d+) tests? run: ")


def nextest(root, out, mode, plan, excluded):
    expr = " | ".join(f"test(={t})" for t in sorted(excluded))
    args = ["--test-threads", "1", "-E", f"not ({expr})" if expr else "all()"]
    env = {**os.environ, "PLAN": plan, "LANE_NEXTEST_DIR": str(out)}
    r = subprocess.run([sys.executable, str(SCRIPT), mode, *args], cwd=root, env=env, capture_output=True, text=True)
    return r, ran_tests(r.stdout)


def check(root, out, plan, excluded, show):
    label = f"plan={plan} excluded={sorted(excluded)}"
    selected = [i for i, t in enumerate(TESTS) if t not in excluded]
    novel = any(plan[i] == "U" for i in selected)
    # nextest exits non-zero when the filter selects no test; the run must not read that as green.
    failed = novel or not selected
    r, done = nextest(root, out, "run", plan, excluded)
    log = f"{label}\n{r.stdout}\n{r.stderr}"
    if show:
        print(f"run {label}\n{r.stdout}{(out / 'reds.txt').read_text()}")
    want = [(TESTS[i], "PASS" if plan[i] == "P" else "FAIL") for i in selected]
    assert done == want, f"{log}\nran {done}, want {want}"
    assert (r.returncode != 0) == failed, f"{log}\nexit {r.returncode}"
    named = [l for l in r.stdout.splitlines() if l.startswith("[lane-nextest] novel: ")]
    assert bool(named) == novel and all(UNLISTED in l for l in named), f"{log}\nnovel lines {named}"
    reds = [t for t, s in done if s == "FAIL"]
    listed = [l.split(" ") for l in (out / "reds.txt").read_text().splitlines()]
    assert listed == [["fixture::main", t] for t in reds], f"{log}\nred list {listed}, want {reds}"
    if not reds:
        rr = subprocess.run([sys.executable, str(SCRIPT), "rerun"], cwd=root, capture_output=True, text=True,
                            env={**os.environ, "LANE_NEXTEST_DIR": str(out)})
        assert rr.returncode != 0 and "no red tests" in rr.stderr, f"{log}\nrerun of an empty list: {rr}"
        return
    r, ran = nextest(root, out, "rerun", plan, excluded)
    if show:
        print(f"rerun {label}\n{r.stdout}")
    summary = [m.group(1) for m in map(SUMMARY.match, r.stdout.splitlines()) if m]
    assert sorted(t for t, _ in ran) == sorted(reds), f"{log}\nrerun ran {ran}, want {reds}\n{r.stdout}\n{r.stderr}"
    assert summary == [str(len(reds))], f"{log}\nrerun summary {summary}\n{r.stdout}"


def draw_excluded(rng):
    """A run filter: a random subset of the tests excluded, the whole set included."""
    return set(rng.sample(TESTS, rng.choice([0, 0, 1, 2, N])))


def draw_killed_plan(rng):
    """A run whose nextest dies at test `i` (`X`, a SIGKILL of nextest) after passes and known reds only."""
    i = rng.randrange(1, N)
    return "".join(rng.choice("PK") for _ in range(i)) + "X" + "P" * (N - i - 1)


def draw_cancelled_plan(rng):
    """A run that nextest cancels at test `i` (`S`, a SIGINT to nextest that the test itself survives and passes)."""
    i = rng.randrange(0, N - 1)
    return "".join(rng.choice("PK") for _ in range(i)) + "S" + "P" * (N - i - 1)


def check_killed(root, out, plan):
    r, _ = nextest(root, out, "run", plan, set())
    assert r.returncode != 0, f"plan={plan}\n{r.stdout}\n{r.stderr}\nexit {r.returncode} for a run whose nextest died"


def main():
    cases = int(sys.argv[1]) if len(sys.argv) > 1 else 24
    rng = random.Random(int(os.environ.get("SEED", random.randrange(1 << 30))))
    generated = [("".join(rng.choice("PPKU") for _ in TESTS), draw_excluded(rng))
                 for _ in range(cases)]
    with tempfile.TemporaryDirectory() as tmp:
        root, out = Path(tmp) / "fixture", Path(tmp) / "out"
        root.mkdir()
        fixture(root, known_red_wording())
        subprocess.run(["cargo", "build", "--tests", "--quiet"], cwd=root, check=True)
        step = subprocess.run([str(REPO / "scripts/flow-checks/run-step.sh"), "nextest: --test-threads 1 -E 'all()'"],
                              cwd=root, capture_output=True, text=True,
                              env={**os.environ, "BASE": "0" * 40, "PLAN": "PPPUPPPP", "SLEEP_AFTER_U": "3",
                                   "LANE_NEXTEST_DIR": str(out)})
        print(step.stdout)
        ran = [m.groups()[::-1] for m in map(STATUS.match, step.stdout.splitlines()) if m]
        assert ran[:N] == [(t, "FAIL" if t == "t03" else "PASS") for t in TESTS], step
        assert (out / "reds.txt").read_text() == "fixture::main t03\n", step
        assert step.stdout.splitlines()[-1] == "exit: 1", step
        for k, (plan, excluded) in enumerate(EXAMPLES + generated):
            check(root, out, plan, excluded, k < len(EXAMPLES))
            print(f"ok plan={plan} excluded={sorted(excluded)}", flush=True)
        for plan in ["KXPPPPPP"] + [draw_killed_plan(rng) for _ in range(max(cases // 4, 2))]:
            check_killed(root, out, plan)
            print(f"ok killed plan={plan}", flush=True)
        for plan in ["KSPPPPPP"] + [draw_cancelled_plan(rng) for _ in range(max(cases // 4, 2))]:
            check_killed(root, out, plan)
            print(f"ok cancelled plan={plan}", flush=True)
    print("ok")


main()
