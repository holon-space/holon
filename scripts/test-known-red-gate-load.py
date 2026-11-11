#!/usr/bin/env python3
"""Self-check for the verdict of scripts/known-red-gate-load.py.

Run: `python3 scripts/test-known-red-gate-load.py` (`SEED=<n>` replays a draw, `CASES=<n>` sets the count).
Generators: a rule (a bound in seconds or a base red rate), a run set (n, seconds in [0, 2x bound], pass or red,
load_max, qualifying or not), a p90 load. Properties P1-P5 of the verdict hold for every draw; the static examples run
first through the same properties, each with its expected outcome.
The gate band: job spans and (minute, load) samples; a sample outside every span, or a span repeated, never moves it.
"""
import importlib.util
import math
import os
import random
import subprocess
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "known-red-gate-load.py"
P90 = 460.0


def load_module():
    spec = importlib.util.spec_from_file_location("known_red_gate_load", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run(seconds, passed=True, load_max=120.0, qualifies=True):
    return {"seconds": seconds, "passed": passed, "load_max": load_max, "qualifies": qualifies}


def five(seconds, load_max=120.0):
    return [run(seconds, load_max=load_max) for _ in range(5)]


# (name, rule, runs, expected outcome: "delete", "keep" or "no verdict")
EXAMPLES = [
    ("composed boot 15.0 s of 60 s", ("bound", 60.0), five(15.0), "delete"),
    ("composed boot at the boundary 30 s of 60 s", ("bound", 60.0), five(30.0), "delete"),
    ("composed boot past the boundary keeps", ("bound", 60.0), five(30.0)[:4] + [run(30.001, load_max=470.0)], "keep"),
    ("a 120 s cap hit keeps", ("bound", 120.0), five(40.0)[:4] + [run(120.0, passed=False, load_max=470.0)], "keep"),
    ("heal-teeth 76.7 s alone keeps under the 120 s cap", ("bound", 120.0),
     five(50.0)[:4] + [run(76.7, load_max=470.0)], "keep"),
    ("a kept row with no run at p90 has no verdict", ("bound", 60.0), five(31.0), "no verdict"),
    ("four qualifying runs have no verdict", ("bound", 60.0), five(1.0)[:4] + [run(1.0, qualifies=False)],
     "no verdict"),
    ("rate 1/2 needs 12 clean runs", ("rate", 0.5), [run(1.0) for _ in range(12)], "delete"),
    ("rate 1/2 with 11 clean runs has no verdict", ("rate", 0.5), [run(1.0) for _ in range(11)], "no verdict"),
    ("a red keeps a rate row", ("rate", 0.5), [run(1.0) for _ in range(4)] + [run(1.0, False, 470.0)], "keep"),
    ("rate 7/51 needs more runs than the cap: kept", ("rate", 7 / 51), [run(1.0) for _ in range(4)] +
     [run(1.0, load_max=470.0)], "keep"),
]


def outcome(m, rule, runs):
    try:
        return m.verdict(rule, runs, P90)
    except m.NoVerdict as e:
        return f"no verdict: {e}"


def kind(result):
    return "no verdict" if result.startswith("no verdict") else result


def check(m, rule, runs, rng):
    got = outcome(m, rule, runs)
    q = [r for r in runs if r["qualifies"]]
    # P1: unqualified runs never contribute.
    noise = [run(rng.uniform(0, 1e4), rng.random() < 0.5, rng.uniform(0, 900), False) for _ in range(rng.randint(1, 4))]
    mixed = runs + noise
    rng.shuffle(mixed)
    assert outcome(m, rule, mixed) == got, f"P1: adding unqualified runs changed {got!r} for {rule} {runs}"
    # P5: fewer than five qualifying runs give no verdict.
    if len(q) < m.MIN_RUNS:
        assert kind(got) == "no verdict", f"P5: {len(q)} qualifying runs gave {got!r}"
    if got == "delete":
        assert len(q) >= m.MIN_RUNS and all(r["passed"] for r in q), f"P2: delete on {q}"
        if rule[0] == "bound":
            assert max(r["seconds"] for r in q) <= rule[1] / 2, f"P2: delete past half the bound {rule} {q}"
        else:
            # P4: a rate row is deleted only after the rule-of-three count of clean runs.
            assert len(q) >= math.ceil(6 / rule[1]), f"P4: {len(q)} runs deleted a row of rate {rule[1]}"
        # P3: lowering any run's seconds cannot turn delete into keep.
        lower = [dict(r, seconds=r["seconds"] * rng.random()) for r in runs]
        assert outcome(m, rule, lower) == "delete", f"P3: lower seconds turned delete into {outcome(m, rule, lower)}"
    return got


def check_band(m, spans, samples, rng):
    inside = [load for t, load in samples if any(a <= t <= b for a, b in spans)]
    if not inside:
        try:
            m.gate_band(spans, samples)
        except ValueError:
            return
        raise AssertionError(f"band: no sample inside {spans} gave a band")
    floor, p90, n = m.gate_band(spans, samples)
    assert n == len(inside) and min(inside) <= floor <= p90 <= max(inside), f"band: {floor} {p90} of {inside}"
    outside = [(t, rng.uniform(0, 900)) for t in range(-50, 0)]
    assert m.gate_band(spans + spans[:1], samples + outside) == (floor, p90, n), "band: outside sample or repeat moved it"


def draw_band(rng):
    spans = [(a, a + rng.randint(0, 60)) for a in (rng.randint(0, 600) for _ in range(rng.randint(0, 6)))]
    return spans, [(t, rng.uniform(0, 500)) for t in range(0, 700, rng.randint(1, 30))]


def draw_unsampled_run(rng):
    """A run process under which no test process with the sampled name ever appears."""
    return rng.choice([["sleep", f"{rng.uniform(0, 0.4):.2f}"], ["/bin/sh", "-c", "true"], ["/bin/sh", "-c", "sleep 0.3 & wait"]])


def check_sampler(m, argv, rng):
    # A cause run whose test process is never sampled has no split: it must fail, not return no reports.
    run = subprocess.Popen(argv)
    try:
        reports = m.sample_until_exit(run, f"never_listed_{rng.randrange(1 << 20)}", Path(tempfile.mkdtemp()) / "c.log")
    except SystemExit:
        return
    finally:
        run.wait()
    raise AssertionError(f"sampler: {argv} gave {reports!r}, a cause run with no sample report")


def check_sampler_unwritten(m, seconds, rng):
    """The test process is found, but exits before `sample` writes its first report (a stub `sample` waits for the
    exit): the cause run has no split and must fail."""
    stub = Path(tempfile.mkdtemp())
    (stub / "sample").write_text('#!/bin/sh\nwhile kill -0 "$1" 2>/dev/null; do sleep 0.05; done\n')
    (stub / "sample").chmod(0o755)
    name = f"exits_unsampled_{rng.randrange(1 << 20)}"
    run = subprocess.Popen(["/bin/sh", "-c", '"$@"; true', "sh", "perl", "-e", "select(undef, undef, undef, shift)",
                            f"{seconds:.2f}", name, "--exact"])
    path = os.environ["PATH"]
    os.environ["PATH"] = f"{stub}:{path}"
    try:
        reports = m.sample_until_exit(run, name, stub / "c.log")
    except SystemExit:
        return
    finally:
        os.environ["PATH"] = path
        run.wait()
    raise AssertionError(f"sampler: a test of {seconds:.2f} s with no written report gave {reports!r}")


def draw(rng):
    if rng.random() < 0.6:
        bound = rng.choice([5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 600.0])
        rule, hi = ("bound", bound), 2 * bound
    else:
        rule, hi = ("rate", rng.choice([1 / 2, 2 / 5, 1 / 4, 7 / 51, 8 / 51, 1.0])), 600.0
    n = rng.randint(0, 45)
    red = rng.choice([0.0, 0.0, 0.05, 0.3])
    low = rng.random() < 0.5
    runs = [run(rng.uniform(0, hi / 2 if low else hi), rng.random() >= red, rng.uniform(80, 700),
                rng.random() < 0.85) for _ in range(n)]
    return rule, runs


def draw_row(rng):
    """A rule and the run sets of one to three tests of one row."""
    rule, runs = draw(rng)
    return rule, [runs] + [draw(rng)[1] for _ in range(rng.randint(0, 2))]


def check_row(m, rule, run_sets):
    per = [kind(outcome(m, rule, runs)) for runs in run_sets]
    try:
        got = m.row_verdict(rule, {str(i): runs for i, runs in enumerate(run_sets)}, P90)
    except m.NoVerdict:
        got = "no verdict"
    want = "keep" if "keep" in per else "no verdict" if "no verdict" in per else "delete"
    assert got == want, f"row: tests {per} gave {got!r}, want {want!r}"


def main():
    m = load_module()
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    rng = random.Random(seed)
    for name, rule, runs, want in EXAMPLES:
        got = check(m, rule, runs, rng)
        assert kind(got) == want, f"example {name!r}: want {want}, got {got!r}"
    for key, status, red_rows, whole, want in [("a", "PASS", "", False, True), ("a", "FAIL", "b", False, True),
                                               ("a", "FAIL", "a,b", False, False), ("a", "FAIL", "novel", False, False),
                                               ("a", "TIMEOUT", "b", False, False), ("a", "PASS", "", True, True),
                                               ("a", "FAIL", "b", True, False)]:
        assert m.passed_for(key, status, red_rows, whole) == want, f"passed_for{(key, status, red_rows, whole)} != {want}"
    report = ("Call graph:\n"
              "    10 Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)\n"
              "    + 10 start  (in dyld) + 1  [0x1]\n"
              "    +   6 __psynch_mutexwait  (in libsystem_kernel.dylib) + 8  [0x2]\n"
              "    +   3 work  (in t) + 4  [0x3]\n"
              "    +   ! 2 kevent  (in libsystem_kernel.dylib) + 8  [0x4]\n"
              "    5 Thread_2: tokio-runtime-worker\n"
              "      5 thread_start  (in libsystem_pthread.dylib) + 8  [0x5]\n"
              "        5 __psynch_cvwait  (in libsystem_kernel.dylib) + 8  [0x6]\n"
              "\nTotal number in stack\n")
    assert m.thread_samples(report) == {
        "Thread_1   DispatchQueue_1: com.apple.main-thread  (serial)": {"lock": 6, "cpu": 2, "wait": 2},
        "Thread_2: tokio-runtime-worker": {"wait": 5}}, m.thread_samples(report)
    assert m.rusage("x\n       31.50 real        12.30 user         3.20 sys\n      77  involuntary context switches\n") \
        == (31.5, 15.5, 77)
    check_band(m, [(0, 10), (5, 20)], [(0, 1.0), (10, 3.0), (20, 2.0), (30, 99.0)], rng)
    assert m.gate_band([(0, 10), (5, 20)], [(0, 1.0), (10, 3.0), (20, 2.0), (30, 99.0)]) == (2.0, 3.0, 3)
    for _ in range(int(os.environ.get("CASES", 2000))):
        check_band(m, *draw_band(rng), rng)
    for _ in range(int(os.environ.get("CASES", 2000))):
        check_row(m, *draw_row(rng))
    for _ in range(5):
        check_sampler(m, draw_unsampled_run(rng), rng)
    for _ in range(3):
        check_sampler_unwritten(m, rng.uniform(0.6, 1.2), rng)
    seen = set()
    for _ in range(int(os.environ.get("CASES", 2000))):
        seen.add(kind(check(m, *draw(rng), rng)))
    assert seen == {"delete", "keep", "no verdict"}, f"the generator reached only {seen} (SEED={seed})"
    print(f"known-red-gate-load self-check: OK ({len(EXAMPLES)} examples, SEED={seed})")


if __name__ == "__main__":
    main()
