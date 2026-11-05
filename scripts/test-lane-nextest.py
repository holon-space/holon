#!/usr/bin/env python3
"""Property check for scripts/lane-nextest.py: a real nextest run over a generated fixture crate.

Run: `python3 scripts/test-lane-nextest.py [cases]`. Builds a one-crate workspace in a temp dir (cargo), then exits
non-zero on the first violation.
The crate's tests t00..t07 run one at a time in name order; the env var PLAN gives each test's outcome:
`P` passes, `K` panics with the wording of a known-red row of docs/Testing/KeystoneKnownReds.md, `U` panics with a
wording no row lists. A test after the first `U` sleeps, so a run that does not stop there is seen.
Generators: the plan (each test P, K or U) and the run filter (a random subset of the tests excluded by `-E`).
Properties, per case: every test the run completes has its planned outcome, the run completes the tests up to the
first `U` among the filtered ones and stops within one more, its red list names exactly its completed red tests,
and the rerun from that list runs exactly the listed tests with the same filter.
A `nextest:` step line of scripts/flow-checks/run-step.sh runs through scripts/lane-nextest.py.
"""
import os
import random
import re
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCRIPT = REPO / "scripts" / "lane-nextest.py"
N = 8
TESTS = [f"t{i:02}" for i in range(N)]
# Fixed cases first: the card's planted red, planted known red, an all-green run, and adjacent reds.
EXAMPLES = [("PPPUPPPP", set()), ("PKPPPKPP", set()), ("PPPPPPPP", set()), ("UPPPPPPP", {"t00"}),
            ("KPKPUPPP", {"t02"}), ("PKKPKUPP", {"t01", "t02"}), ("KKKPPPPP", set()),
            ("PKUUPUPU", {"t07"}), ("PKPKPPPU", set())]
UNLISTED = "planted red outside the known-reds list"
STATUS = re.compile(r"^ *([A-Z][A-Z0-9 +/-]*?) +\[[^]]*\] (?:\([^)]*\) )?\S+ (t\d\d)$")
SUMMARY = re.compile(r"^ *Summary \[.*\] (\d+) tests? run: ")


def known_red_wording():
    rows = subprocess.run([sys.executable, str(REPO / "scripts/check-known-red-evidence.py"), "--rows",
                           str(REPO / "docs/Testing/KeystoneKnownReds.md")], capture_output=True, text=True,
                          check=True).stdout
    for row in rows.splitlines():
        key, pattern, site, _ = row.split("\x1f")
        if not site and re.fullmatch(r"[A-Za-z0-9 _'-]+", pattern) and not pattern.startswith("not classifier"):
            return pattern
    raise AssertionError("no unpinned known-red row with a literal pattern to plant")


def fixture(root, wording):
    (root / "tests").mkdir()
    (root / "Cargo.toml").write_text('[package]\nname = "fixture"\nversion = "0.0.0"\nedition = "2021"\n\n'
                                     '[workspace]\n')
    (root / "src").mkdir()
    (root / "src/lib.rs").write_text("")
    tests = "".join(f"#[test]\nfn {t}() {{\n    act({i});\n}}\n\n" for i, t in enumerate(TESTS))
    (root / "tests/main.rs").write_text(f"""fn act(i: usize) {{
    let plan = std::env::var("PLAN").unwrap();
    if plan[..i].contains('U') {{
        let secs = std::env::var("SLEEP_AFTER_U").map_or(30, |s| s.parse().unwrap());
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }}
    match &plan[i..=i] {{
        "P" => {{}}
        "K" => panic!("{wording}"),
        "U" => panic!("{UNLISTED}"),
        o => unreachable!("{{o}}"),
    }}
}}

{tests}""")


def nextest(root, out, mode, plan, excluded):
    expr = " | ".join(f"test(={t})" for t in sorted(excluded))
    args = ["--test-threads", "1", "-E", f"not ({expr})" if expr else "all()"]
    env = {**os.environ, "PLAN": plan, "LANE_NEXTEST_DIR": str(out)}
    r = subprocess.run([sys.executable, str(SCRIPT), mode, *args], cwd=root, env=env, capture_output=True, text=True)
    lines = r.stdout.splitlines()
    # nextest repeats the red status lines in its closing summary.
    end = next((i for i, l in enumerate(lines) if re.match(r"^ *(Summary|Cancelling|Canceling) ", l)), len(lines))
    ran = [(t, s) for t, s in (m.groups()[::-1] for m in map(STATUS.match, lines[:end]) if m)]
    return r, ran


def check(root, out, plan, excluded, show):
    label = f"plan={plan} excluded={sorted(excluded)}"
    selected = [i for i, t in enumerate(TESTS) if t not in excluded]
    first_u = next((k for k, i in enumerate(selected) if plan[i] == "U"), None)
    r, ran = nextest(root, out, "run", plan, excluded)
    log = f"{label}\n{r.stdout}\n{r.stderr}"
    if show:
        print(f"run {label}\n{r.stdout}{(out / 'reds.txt').read_text()}")
    done = [(t, s) for t, s in ran if s != "SIGTERM"]
    in_flight = ran[len(done):]
    # Tests after the first U may complete while the classifier runs; each still matches its plan.
    want = [(TESTS[i], "PASS" if plan[i] == "P" else "FAIL") for i in selected]
    assert done == want[:len(done)], f"{log}\nran {ran}, want a prefix of {want}"
    if first_u is None:
        assert done == want and not in_flight, f"{log}\nran {ran}, want {want}"
    else:
        assert len(done) > first_u, f"{log}\nstopped before the first unlisted red: {ran}"
        assert [t for t, _ in in_flight] in ([], [want[len(done)][0]] if len(done) < len(want) else []), \
            f"{log}\nnot one test in flight: {in_flight}"
        # A test after the first U sleeps 30 s, so a run that reaches two of them did not stop.
        assert len(done) <= first_u + 2, f"{log}\nran past the stop: {ran}"
        assert "[fail-fast] stopped at fixture::main " + want[first_u][0] in r.stdout, log
    assert (r.returncode == 0) == (first_u is None and all(s == "PASS" for _, s in done)), f"{log}\nexit {r.returncode}"
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


def main():
    cases = int(sys.argv[1]) if len(sys.argv) > 1 else 24
    rng = random.Random(int(os.environ.get("SEED", random.randrange(1 << 30))))
    generated = [("".join(rng.choice("PPKU") for _ in TESTS), set(rng.sample(TESTS, rng.choice([0, 0, 1, 2]))))
                 for _ in range(cases)]
    with tempfile.TemporaryDirectory() as tmp:
        root, out = Path(tmp) / "fixture", Path(tmp) / "out"
        root.mkdir()
        fixture(root, known_red_wording())
        subprocess.run(["cargo", "build", "--tests", "--quiet"], cwd=root, check=True)
        step = subprocess.run([str(REPO / "scripts/flow-checks/run-step.sh"), "nextest: --test-threads 1 -E 'all()'"],
                              cwd=root, capture_output=True, text=True,
                              env={**os.environ, "BASE": "0" * 40, "PLAN": "PPPUPPPP", "LANE_NEXTEST_DIR": str(out)})
        print(step.stdout)
        assert "[fail-fast] stopped at fixture::main t03" in step.stdout, step
        assert step.stdout.splitlines()[-1] == "exit: 1", step
        # Tests after the unlisted red finish while the classifier runs, and nextest exits before the stop.
        fast = subprocess.run([sys.executable, str(SCRIPT), "run", "--test-threads", "1", "-E", "all()"], cwd=root,
                              capture_output=True, text=True,
                              env={**os.environ, "PLAN": "PPPUPPPP", "SLEEP_AFTER_U": "0", "LANE_NEXTEST_DIR": str(out)})
        assert "[fail-fast] stopped at fixture::main t03" in fast.stdout, fast
        assert (out / "reds.txt").read_text() == "fixture::main t03\n", fast
        for k, (plan, excluded) in enumerate(EXAMPLES + generated):
            check(root, out, plan, excluded, k < len(EXAMPLES))
            print(f"ok plan={plan} excluded={sorted(excluded)}", flush=True)
    print("ok")


main()
