#!/usr/bin/env python3
"""Property check for scripts/lane-nextest.py: a real nextest run over a generated fixture crate.

Run: `python3 scripts/test-lane-nextest.py [cases]`. Builds a one-crate workspace in a temp dir (cargo), then exits
non-zero on the first violation.
The crate's tests t00..t07 run one at a time in name order; the env var PLAN gives each test's outcome:
`P` passes, `K` panics with the wording of a known-red row of docs/Testing/KeystoneKnownReds.md, `U` panics with a
wording no row lists, `C` prints the captured block and then fails like `K`, `D` prints it and passes.
Generators: the plan (each test P, K, U, C or D), the run filter (a random subset of the tests excluded by `-E`), the
captured block (1-3 lines of the pool of lines that imitate nextest's own, each indented 0-14 columns) and whether
passing tests' output reaches the stream (`--success-output immediate`).
A captured line that sits at the column nextest's own line of that label has (after nextest's own 4-column indent
of captured output) is excluded by construction: the runner reads it as nextest's and this test does not claim
otherwise. Everything else a test prints must change neither the red list nor where the run ends.
The nextest calls of each case see NEXTEST_NO_TESTS=pass, so a run that selects no test fails only through `--no-tests=fail`.
Properties, per case: the run completes every filtered test with its planned outcome, exits non-zero iff a `U` ran
and names a novel signature iff it did, its red list names exactly its red tests, and the rerun from that list runs
exactly the listed tests with the same filter.
Fixed plans run first through the same properties: the orders of red kinds a run can end in (a novel red last, a
novel red before a known one, no red) and earlier finds.
A run that reads a red prints the classifier's verdict file whole, as one block, so a step log holds it; a run with
no red writes no verdict file and prints no verdict line.
A `nextest:` step line of scripts/flow-checks/run-step.sh runs through scripts/lane-nextest.py, and runs on past a
novel red whose later tests take seconds each. Its step log holds the verdict block below `TREE:`, and the classifier
judges that step log as it judges the run's own stream log, for a novel red and for known reds only.
"""
import importlib.util
import os
import random
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from nextest_fixture import (CAPTURE_INDENT, N, POOL, REPO, TESTS, UNLISTED, captured_line, fixture, forges,
                             known_red_wording, ran_tests)

SCRIPT = REPO / "scripts" / "lane-nextest.py"
CLASSIFIER = REPO / "scripts" / "keystone-known-reds.sh"
PLAIN = [("PPPUPPPP", set()), ("PKPPPKPP", set()), ("PPPPPPPP", set()), ("UPPPPPPP", {"t00"}),
         ("KPKPUPPP", {"t02"}), ("PKKPKUPP", {"t01", "t02"}), ("KKKPPPPP", set()),
         ("PKUUPUPU", {"t07"}), ("PKPKPPPU", set()), ("PPPPPPPU", set()), ("PPUPKPPP", set()),
         ("PPPPPPPP", set(TESTS))]
ALL_POOL = [0, 1, 2]
CAPTURE_COLUMNS = [0, 1, 4, 5, 8, 12]
CAPTURE_PLANS = ["PCPUPPPP", "PCUPCPCP"]
# Blocks of captured lines: the card's `    Summary` line, then every pool line at each static column.
CARD_BLOCK = ["    " + POOL[0][1]]
STATIC_BLOCKS = [[captured_line(p, k) for p in ALL_POOL if not forges(POOL[p][0], k)] for k in CAPTURE_COLUMNS]
# Case: (plan, excluded tests, captured block, passing tests' output on the stream).
EXAMPLES = [(plan, excluded, [], False) for plan, excluded in PLAIN]
EXAMPLES += [("PCPUPPPP", set(), CARD_BLOCK, False), ("PDPUPPPP", set(), CARD_BLOCK, True),
             ("PDPUPPPP", set(), CARD_BLOCK, False)]
EXAMPLES += [(plan, set(), block, True) for block in STATIC_BLOCKS for plan in CAPTURE_PLANS]
EXAMPLES += [("CCCCCCCC", {"t00", "t07"}, STATIC_BLOCKS[0], False)]
NO_TESTS = "--no-tests=fail"
SUMMARY = re.compile(r"^ {5}Summary \[.*\] (\d+) tests? run: ")


def nextest(root, out, mode, plan, excluded, block=(), immediate=False):
    expr = " | ".join(f"test(={t})" for t in sorted(excluded))
    args = ["--test-threads", "1", NO_TESTS, "-E", f"not ({expr})" if expr else "all()"]
    if immediate:
        args += ["--success-output", "immediate"]
    env = {**os.environ, "PLAN": plan, "CAPTURE": "\n".join(block), "LANE_NEXTEST_DIR": str(out),
           "NEXTEST_NO_TESTS": "pass"}
    r = subprocess.run([sys.executable, str(SCRIPT), mode, *args], cwd=root, env=env, capture_output=True, text=True)
    return r, ran_tests(r.stdout)


def block_at(lines, block):
    """The index where `block` starts as a contiguous run of `lines`, else None."""
    return next((i for i in range(len(lines) - len(block) + 1) if lines[i:i + len(block)] == block), None)


def check_verdict_printed(out, stdout, reds, log):
    verdict = out / "verdict.log"
    if reds:
        block = verdict.read_text().splitlines()
        assert block and block_at(stdout.splitlines(), block) is not None, \
            f"{log}\nthe verdict file is not one block of stdout:\n{verdict.read_text()}"
    else:
        assert not verdict.exists(), f"{log}\na run with no red wrote {verdict}"
        printed = [l for l in stdout.splitlines() if l.startswith("[known-reds]")]
        assert all(l.startswith("[known-reds] STALE: ") for l in printed), f"{log}\nverdict lines of a green run {printed}"


def judged(log):
    """(exit, verdict word) of the classifier on `log`: the word of its last line."""
    r = subprocess.run([str(CLASSIFIER), str(log)], capture_output=True, text=True)
    return r.returncode, r.stdout.splitlines()[-1].removeprefix("[known-reds] ").split(":")[0]


def check(root, out, case, show):
    plan, excluded, block, immediate = case
    label = f"plan={plan} excluded={sorted(excluded)} block={block} immediate={immediate}"
    selected = [i for i, t in enumerate(TESTS) if t not in excluded]
    novel = any(plan[i] == "U" for i in selected)
    failed = novel or not selected
    r, done = nextest(root, out, "run", plan, excluded, block, immediate)
    log = f"{label}\n{r.stdout}\n{r.stderr}"
    if show:
        print(f"run {label}\n{r.stdout}{(out / 'reds.txt').read_text()}")
    printed = any(plan[i] == "C" or (plan[i] == "D" and immediate) for i in selected)
    stream = r.stdout.splitlines()
    assert not printed or all(" " * CAPTURE_INDENT + line in stream for line in block), \
        f"{log}\nthe captured block is not on the stream {CAPTURE_INDENT} columns further right"
    want = [(TESTS[i], "PASS" if plan[i] in "PD" else "FAIL") for i in selected]
    assert done == want, f"{log}\nran {done}, want {want}"
    assert (r.returncode != 0) == failed, f"{log}\nexit {r.returncode}"
    assert selected or r.returncode == 4, f"{log}\nexit {r.returncode} for a run that selects no test, not 4"
    named = [l for l in r.stdout.splitlines() if l.startswith("[lane-nextest] novel: ")]
    assert bool(named) == novel and all(UNLISTED in l for l in named), f"{log}\nnovel lines {named}"
    reds = [TESTS[i] for i in selected if plan[i] in "KUC"]
    listed = [l.split(" ") for l in (out / "reds.txt").read_text().splitlines()]
    assert ["other::bin", "ghost"] not in listed, f"{log}\nred list {listed} holds a test that never ran"
    assert listed == [["fixture::main", t] for t in reds], f"{log}\nred list {listed}, want {reds}"
    check_verdict_printed(out, r.stdout, reds, log)
    if not reds:
        rr = subprocess.run([sys.executable, str(SCRIPT), "rerun", NO_TESTS], cwd=root, capture_output=True, text=True,
                            env={**os.environ, "LANE_NEXTEST_DIR": str(out)})
        assert rr.returncode != 0 and "no red tests" in rr.stderr, f"{log}\nrerun of an empty list: {rr}"
        return
    r, ran = nextest(root, out, "rerun", plan, excluded, block, immediate)
    if show:
        print(f"rerun {label}\n{r.stdout}")
    summary = [m.group(1) for m in map(SUMMARY.match, r.stdout.splitlines()) if m]
    assert sorted(t for t, _ in ran) == sorted(reds), f"{log}\nrerun ran {ran}, want {reds}\n{r.stdout}\n{r.stderr}"
    assert summary == [str(len(reds))], f"{log}\nrerun summary {summary}\n{r.stdout}"


def draw_block(rng):
    """1-3 captured lines of the pool, each at a column that does not forge nextest's own line."""
    block, size = [], rng.randint(1, 3)
    while len(block) < size:
        p, k = rng.choice(ALL_POOL), rng.randrange(15)
        if not forges(POOL[p][0], k):
            block.append(captured_line(p, k))
    return block


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


def load_runner():
    spec = importlib.util.spec_from_file_location("lane_nextest", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


PASSING_LABELS = ["START", "SLOW", "TRMNTG", "TERMINATING", "PASS", "LEAK", "TMPASS", "TIMEOUT-PASS", "SLOW + LEAK",
                  "SLOW+TMPASS", "SKIP", "FLAKY 2/3", "TRY 2 PASS", "SETUP", "SETUP SLOW", "SETUP PASS"]
RED_LABELS = ["FAIL", "TIMEOUT", "SIGSEGV", "ABORT", "EXECFAIL", "LEAK-FAIL", "FAIL + LEAK", "TRY 2 FAIL",
              "TRY 1 TIMEOUT", "SETUP FAIL", "LONG-FAIL-LABEL"]


def check_labels(rng):
    """A status line is read at nextest's column only, for every label, short or longer than the 12 columns."""
    runner = load_runner()
    for label in PASSING_LABELS + RED_LABELS:
        want = ("other::bin", "t00") if label in RED_LABELS else None
        right = 0 if len(label) >= 12 else 12 - len(label)
        for indent in sorted({right, *(rng.randrange(0, 20) for _ in range(6)), 0, 1, 4, 5, 8, 12}):
            line = f"{' ' * indent}{label} [   0.004s] (2/5) other::bin t00"
            got = runner.red_of(line)
            assert got == (want if indent == right else None), f"{line!r}: red_of {got}, want {want} at column {right}"


def check_end_of_run(rng):
    """The closing summary and the cancel line count at nextest's column only."""
    runner = load_runner()
    for word, indent in [("Summary", 5), ("Cancelling", 2), ("Canceling", 3)]:
        for k in sorted({indent, *(rng.randrange(0, 16) for _ in range(6)), 0, 1, 4, 5, 8, 12}):
            line = f"{' ' * k}{word} due to test failure"
            assert bool(runner.END_OF_RUN.match(line)) == (k == indent), f"{line!r}: END_OF_RUN"
            assert bool(runner.CANCELLED.match(line)) == (k == indent and word != "Summary"), f"{line!r}: CANCELLED"


def check_killed(root, out, plan):
    r, _ = nextest(root, out, "run", plan, set())
    assert r.returncode != 0, f"plan={plan}\n{r.stdout}\n{r.stderr}\nexit {r.returncode} for a run whose nextest died"


def main():
    cases = int(sys.argv[1]) if len(sys.argv) > 1 else 24
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    print(f"seed {seed}", flush=True)
    rng = random.Random(seed)
    generated = [("".join(rng.choice("PPKUCD") for _ in TESTS), draw_excluded(rng), draw_block(rng),
                  rng.random() < 0.5) for _ in range(cases)]
    with tempfile.TemporaryDirectory() as tmp:
        root, out = Path(tmp) / "fixture", Path(tmp) / "out"
        root.mkdir()
        fixture(root, known_red_wording())
        subprocess.run(["cargo", "build", "--tests", "--quiet"], cwd=root, check=True)
        for plan, exit_line in [("PPPUPPPP", "exit: 1"), ("KPKPPPPP", "exit: 0")]:
            step = subprocess.run([str(REPO / "scripts/flow-checks/run-step.sh"), "nextest: --test-threads 1 --no-tests=fail -E 'all()'"],
                                  cwd=root, capture_output=True, text=True,
                                  env={**os.environ, "BASE": "0" * 40, "PLAN": plan, "SLEEP_AFTER_U": "3",
                                       "LANE_NEXTEST_DIR": str(out)})
            print(step.stdout)
            ran = ran_tests(step.stdout)
            reds = [t for t, p in zip(TESTS, plan) if p != "P"]
            assert ran[:N] == [(t, "FAIL" if t in reds else "PASS") for t in TESTS], step
            assert (out / "reds.txt").read_text() == "".join(f"fixture::main {t}\n" for t in reds), step
            lines = step.stdout.splitlines()
            assert lines[-1] == exit_line, step
            at = block_at(lines, (out / "verdict.log").read_text().splitlines())
            tree = next(i for i, l in enumerate(lines) if l.startswith("TREE: "))
            assert at is not None and tree < at, f"{step}\nthe verdict block is not in the step log below TREE:"
            step_log = Path(tmp) / "step.log"
            step_log.write_text(step.stdout)
            assert judged(step_log) == judged(out / "stream.log"), \
                f"{step}\nthe classifier judges the step log {judged(step_log)}, the stream log {judged(out / 'stream.log')}"
        for args in [[], ["-E", "all()"], [NO_TESTS, NO_TESTS], ["--no-tests=pass"], ["--no-tests", "fail"]]:
            for mode in ("run", "rerun"):
                r = subprocess.run([sys.executable, str(SCRIPT), mode, *args], cwd=root, capture_output=True, text=True)
                assert r.returncode == 1 and "--no-tests=fail exactly once" in r.stderr, f"{mode} {args}: {r}"
        for k, case in enumerate(EXAMPLES + generated):
            check(root, out, case, k < len(EXAMPLES))
            print(f"ok {case}", flush=True)
        for plan in ["KXPPPPPP"] + [draw_killed_plan(rng) for _ in range(max(cases // 4, 2))]:
            check_killed(root, out, plan)
            print(f"ok killed plan={plan}", flush=True)
        for plan in ["KSPPPPPP"] + [draw_cancelled_plan(rng) for _ in range(max(cases // 4, 2))]:
            check_killed(root, out, plan)
            print(f"ok cancelled plan={plan}", flush=True)
    check_labels(rng)
    check_end_of_run(rng)
    print("ok")


if __name__ == "__main__":
    main()
