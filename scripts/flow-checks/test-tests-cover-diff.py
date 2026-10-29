#!/usr/bin/env python3
"""Self-check for scripts/flow-checks/tests-cover-diff.py.

Run: `python3 scripts/flow-checks/test-tests-cover-diff.py [cases]`. Exits non-zero on the first violation.
Each fixture is a lane root in a temp dir whose `scripts/select-tests.py` prints a fixed selection.
fixtures/run-step-clippy.txt is a log scripts/flow-checks/run-step.sh wrote for a real lane.
"""
import importlib.util
import random
import subprocess
import sys
import tempfile
from pathlib import Path

CHECKER = Path(__file__).resolve().parent / "tests-cover-diff.py"
REAL_LOG = Path(__file__).resolve().parent / "fixtures/run-step-clippy.txt"
HEADER = "BASE: 3936d83aba008d69cc09e73d1c5db883ed6b5ae4\nTREE: e5d2c227aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
CLIPPY = "clippy: --workspace --features pbt"
NEXTEST = "nextest: --workspace --features pbt -E 'package(holon) | package(holon-api)' --profile load-tolerant"
NARROW = "nextest: --workspace --features pbt -E 'package(holon)' --profile load-tolerant"
WINDOWED = "windowed: just windowed-gate"
SELECTED = [CLIPPY, NEXTEST, WINDOWED]
TIMING = "timing: just timing-gate"
UNOWNED = "unowned: package(leaf) is excluded by the load-tolerant profile and has no recipe in scripts/timing-split.py"


def step_log(line, code=0):
    return f"{line}\n{HEADER}running\nSummary [ 1s]\n\nexit: {code}\n"


def run(selected, logs, spans=(), selector_exit=0, ran={}):
    """Run the checker in a fresh lane root; `logs` maps a path under target/ to the step lines of a log shaped like
    the selector output, `ran` maps a path under target/ to the text of a run-step log."""
    with tempfile.TemporaryDirectory() as root:
        root = Path(root)
        (root / "scripts").mkdir()
        out = HEADER + "".join(line + "\n" for line in selected)
        (root / "scripts/select-tests.py").write_text(
            f"import sys\nsys.stdout.write({out!r})\nprint('selector failed', file=sys.stderr)\n"
            f"sys.exit({selector_exit})\n")
        for path, lines in logs.items():
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_text(HEADER + "running\n" + "".join(line + "\n" for line in lines) + "Summary [ 1s]\n")
        for path, text in ran.items():
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            (root / path).write_text(text)
        summary = root / "target/summary.md"
        summary.parent.mkdir(exist_ok=True)
        summary.write_text("# lane change summary\n\nHow tested:\n"
                           + "".join(f"- Green log: {p} (`Summary [ 1s]`)\n" for p in [*logs, *ran])
                           + "".join(f"- selector line `{s}`\n" for s in spans)
                           + "\nConfidence (1-5) that the verifier will confirm: 4\n")
        return subprocess.run([sys.executable, str(CHECKER), "--base", "3936d83", "--summary", str(summary)],
                              cwd=root, capture_output=True, text=True)


def expect(name, r, code, *needles):
    assert (r.returncode == 0) == (code == 0), f"{name}: exit {r.returncode}, stderr:\n{r.stderr}"
    for n in needles:
        assert n in r.stderr, f"{name}: {n!r} not on stderr:\n{r.stderr}"
    print(f"[tests-cover-diff] {name}: {'PASS' if code == 0 else 'FAIL'} as expected")


def fixtures():
    ran = {f"target/r1/{line.split(':')[0]}.log": step_log(line) for line in SELECTED}
    expect("pass", run(SELECTED, {"target/r1/select.log": SELECTED}, spans=[NEXTEST, "nextest:"], ran=ran), 0)
    real = REAL_LOG.read_text()
    real_line = real.splitlines()[0]
    expect("real run-step log", run([real_line], {}, ran={"target/r2/clippy.log": real}), 0)
    expect("real run-step log ending exit: 1",
           run([real_line], {}, ran={"target/r2/clippy.log": real.replace("exit: 0", "exit: 1")}),
           1, "target/r2/clippy.log", "exit: 1")
    expect("disclosure lines need no log",
           run([real_line, TIMING, UNOWNED], {}, spans=[TIMING], ran={"target/r2/clippy.log": real}), 0)
    expect("run-step log without exit trailer",
           run(SELECTED, {}, ran={**ran, "target/r1/nextest.log": step_log(NEXTEST).replace("exit: 0\n", "")}),
           1, "target/r1/nextest.log", "`nextest`")
    expect("narrower set",
           run(SELECTED, {"target/r1/select.log": [CLIPPY, WINDOWED], "target/r1/nextest.log": [NARROW]}),
           1, "target/r1/nextest.log", NARROW, NEXTEST)
    expect("also a narrower line", run(SELECTED, {"target/r1/select.log": SELECTED + [NARROW]}),
           1, "target/r1/select.log", NARROW)
    # The selector's own output names the steps but is no evidence that any of them ran.
    expect("selector output only", run(SELECTED, {"target/r1/select.log": SELECTED}), 1)
    expect("step not covered", run(SELECTED, {"target/r1/select.log": [CLIPPY, WINDOWED]}), 1, "`nextest`")
    expect("changed span", run(SELECTED, {"target/r1/select.log": SELECTED}, spans=[NARROW]), 1, f"`{NARROW}`")
    expect("step printed twice", run(SELECTED + [NARROW], {"target/r1/select.log": SELECTED + [NARROW]}),
           1, "step `nextest` 2 times")
    expect("selector fails", run(SELECTED, {"target/r1/select.log": SELECTED}, selector_exit=3),
           1, "exited 3", "selector failed")


def load_checker():
    spec = importlib.util.spec_from_file_location("tests_cover_diff", CHECKER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def generated(cases):
    """Random selections with one run-step log per run step: the exact set passes, each single mutation fails."""
    checker = load_checker()
    check, not_run = checker.check, checker.not_run_steps()
    rng = random.Random(0)
    names = ["clippy", "nextest", "windowed", "keystone-smoke", "timing", "unowned", "a", "b-2"]
    tokens = ["--workspace", "--features", "pbt", "-E", "'package(x)'", "just", "gate", "binary(y)",
              "a:", "b-2: x", "\\t", "  ", "é"]
    # No noise line is exactly `exit: 0`: run-step.sh writes that line, cargo and just never print it.
    noise = ["", "error: unrelated", "Summary [ 1s] 3 tests run", "nextest:", "UPPER: x", "exit: 1", "  exit: 0"]
    for case in range(cases):
        steps = rng.sample(names, rng.randint(1, len(names)))
        # The selector's own output contract (scripts/select-tests.py STEP_LINE) starts the args with a non-blank.
        selected = {s: f"{s}: --workspace " + " ".join(rng.choices(tokens, k=rng.randint(0, 6))) for s in steps}
        stdout = HEADER + "".join(line + "\n" for line in selected.values())
        logs = {f"target/r1/{s}.log": "\n".join([line, *HEADER.splitlines(), *rng.sample(noise, rng.randint(0, 7)),
                                                 "", "exit: 0"]) for s, line in selected.items() if s not in not_run}
        if rng.random() < 0.5:
            logs["target/r1/select.log"] = stdout
        spans = "".join(f"`{line}`\n" for line in selected.values())
        bad = check(stdout, spans, logs, not_run)
        assert bad == [], f"case {case}: exact set fails: {bad}"

        step = rng.choice(steps)
        mutated = selected[step] + " extra"
        assert check(stdout, spans.replace(selected[step], mutated), logs, not_run), f"case {case}: changed span passes"
        assert check(stdout + selected[step] + "\n", spans, logs, not_run), f"case {case}: step printed twice passes"
        if step in not_run:
            continue
        path = f"target/r1/{step}.log"
        for name, text in [("changed line 1", logs[path].replace(selected[step], mutated, 1)),
                           ("exit 1", logs[path].rsplit("exit: 0", 1)[0] + "exit: 1"),
                           ("no exit trailer", logs[path].rsplit("exit: 0", 1)[0])]:
            assert any(path in b for b in check(stdout, spans, {**logs, path: text}, not_run)), \
                f"case {case}: {name} in {path} passes"
        dropped = {p: t for p, t in logs.items() if p != path}
        assert any(f"`{step}`" in b for b in check(stdout, spans, dropped, not_run)), \
            f"case {case}: dropped {step} passes"
    print(f"[tests-cover-diff] generated: {cases} cases PASS")


if __name__ == "__main__":
    fixtures()
    generated(int(sys.argv[1]) if len(sys.argv) > 1 else 300)
