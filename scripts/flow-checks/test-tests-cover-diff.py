#!/usr/bin/env python3
"""Self-check for scripts/flow-checks/tests-cover-diff.py.

Run: `python3 scripts/flow-checks/test-tests-cover-diff.py [cases]`. Exits non-zero on the first violation.
Each fixture is a lane root in a temp dir whose `scripts/select-tests.py` prints a fixed selection.
"""
import importlib.util
import random
import subprocess
import sys
import tempfile
from pathlib import Path

CHECKER = Path(__file__).resolve().parent / "tests-cover-diff.py"
HEADER = "BASE: 3936d83aba008d69cc09e73d1c5db883ed6b5ae4\nTREE: e5d2c227aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
CLIPPY = "clippy: --workspace --features pbt"
NEXTEST = "nextest: --workspace --features pbt -E 'package(holon) | package(holon-api)' --profile load-tolerant"
NARROW = "nextest: --workspace --features pbt -E 'package(holon)' --profile load-tolerant"
WINDOWED = "windowed: just windowed-gate"
SELECTED = [CLIPPY, NEXTEST, WINDOWED]


def run(selected, logs, spans=(), selector_exit=0):
    """Run the checker in a fresh lane root; `logs` maps a path under target/ to its step lines."""
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
        summary = root / "target/summary.md"
        summary.parent.mkdir(exist_ok=True)
        summary.write_text("# lane change summary\n\nHow tested:\n"
                           + "".join(f"- Green log: {p} (`Summary [ 1s]`)\n" for p in logs)
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
    expect("pass", run(SELECTED, {"target/r1/select.log": SELECTED, "target/r1/nextest.log": [NEXTEST]},
                       spans=[NEXTEST, "nextest:"]), 0)
    expect("narrower set",
           run(SELECTED, {"target/r1/select.log": [CLIPPY, WINDOWED], "target/r1/nextest.log": [NARROW]}),
           1, "target/r1/nextest.log", NARROW, NEXTEST)
    expect("also a narrower line", run(SELECTED, {"target/r1/select.log": SELECTED + [NARROW]}),
           1, "target/r1/select.log", NARROW)
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
    """Random selections spread over random logs: the exact copy passes, each single mutation fails."""
    check = load_checker().check
    rng = random.Random(0)
    names = ["clippy", "nextest", "windowed", "keystone-smoke", "unowned", "a", "b-2"]
    tokens = ["--workspace", "--features", "pbt", "-E", "'package(x)'", "just", "gate", "binary(y)",
              "a:", "b-2: x", "\\t", "  ", "é"]
    for case in range(cases):
        steps = rng.sample(names, rng.randint(1, len(names)))
        # The selector's own output contract (scripts/select-tests.py STEP_LINE) starts the args with a non-blank.
        selected = {s: f"{s}: --workspace " + " ".join(rng.choices(tokens, k=rng.randint(0, 6))) for s in steps}
        stdout = HEADER + "".join(line + "\n" for line in selected.values())
        logs = {f"target/r1/l{i}.log": [] for i in range(rng.randint(1, 4))}
        for line in selected.values():
            for path in rng.sample(sorted(logs), rng.randint(1, len(logs))):
                logs[path].append(line)
        noise = ["", "error: unrelated", "Summary [ 1s] 3 tests run", "nextest:", "UPPER: x"]
        text = {p: "\n".join(rng.sample(lines + noise, len(lines) + len(noise))) for p, lines in logs.items()}
        spans = "".join(f"`{line}`\n" for line in selected.values())
        assert check(stdout, spans, text) == [], f"case {case}: exact copy fails: {check(stdout, spans, text)}"

        step = rng.choice(steps)
        holder = next(p for p, lines in logs.items() if selected[step] in lines)
        mutated = selected[step] + " extra"
        changed = {**text, holder: text[holder].replace(selected[step], mutated)}
        assert any(holder in b for b in check(stdout, spans, changed)), f"case {case}: changed line in {holder} passes"
        dropped = {p: "\n".join(line for line in t.splitlines() if line != selected[step]) for p, t in text.items()}
        assert any(f"`{step}`" in b for b in check(stdout, spans, dropped)), f"case {case}: dropped {step} passes"
        assert check(stdout, spans.replace(selected[step], mutated), text), f"case {case}: changed span passes"
        assert check(stdout + selected[step] + "\n", spans, text), f"case {case}: step printed twice passes"
    print(f"[tests-cover-diff] generated: {cases} cases PASS")


if __name__ == "__main__":
    fixtures()
    generated(int(sys.argv[1]) if len(sys.argv) > 1 else 300)
