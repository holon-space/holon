#!/usr/bin/env python3
"""Properties of the `unit-log:` lines of scripts/windowed-gate.py: a run prints one per log of every unit it leaves
RED or HUNG (first run and rerun), each with the hash `shasum -a 256` gives for that file, and none for any other unit.

Run: `python3 scripts/test-windowed-gate-unit-logs.py` (CASES=<n>, SEED=<s>). The fixed examples run first.
main() runs with the cargo calls and the ledger replaced: each unit's run writes a log and returns its drawn verdict.
"""
import contextlib
import importlib.util
import io
import os
import random
import re
import subprocess
import sys
import tempfile
from pathlib import Path

GATE = Path(__file__).resolve().parent / "windowed-gate.py"
UNIT_LOG = re.compile(r"  unit-log: (.+) (\S+) sha256=([0-9a-f]{64})")
FIRST = ["PASS", "KNOWN-RED", "RED", "HUNG"]
RERUN = ["PASS", "KNOWN-RED", "RED", "HUNG"]


def load_gate():
    spec = importlib.util.spec_from_file_location("windowed_gate", GATE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run_gate(plan, log_dir):
    """Runs main() over `plan` ({unit: (first verdict, rerun verdict)}); returns (exit, stdout)."""
    gate = load_gate()
    gate.build = lambda features, d: []
    gate.inventory = lambda reuse: [(unit, f"binary_id(={unit})", 1) for unit in sorted(plan)]
    gate.record_and_report = lambda results: None

    def run_one(unit, filt, tests, reuse, d, timeout, suffix=""):
        log = d / (unit.replace("::", "__").replace(" ", "__") + suffix + ".log")
        log.write_text(f"{unit}{suffix}: {random.random()}\n")
        verdict = plan[unit][1 if suffix else 0]
        return {"binary": unit, "filter": filt, "tests": tests, "wall": 0.0, "verdict": verdict, "log": log,
                "orphans_killed": 0}

    gate.run_one = run_one
    out, err = io.StringIO(), io.StringIO()
    sys.argv = ["windowed-gate.py", "--features", "x", "--log-dir", str(log_dir)]
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = gate.main()
    return code, out.getvalue()


def shasum(path):
    return subprocess.run(["shasum", "-a", "256", str(path)], check=True, capture_output=True,
                          text=True).stdout.split()[0]


def check(name, plan):
    with tempfile.TemporaryDirectory() as d:
        log_dir = Path(d)
        code, out = run_gate(plan, log_dir)
        bad = {u for u, (first, rerun) in plan.items() if first in ("RED", "HUNG") and rerun in ("RED", "HUNG")}
        stem = {u: str(log_dir / u.replace("::", "__").replace(" ", "__")) for u in plan}
        want = {(u, f"{stem[u]}{suffix}.log") for u in bad for suffix in ("", ".rerun")}
        lines = [m.groups() for line in out.splitlines() if (m := UNIT_LOG.fullmatch(line))]
        assert {(u, p) for u, p, _ in lines} == want and len(lines) == len(want), (name, plan, lines, want)
        for u, p, sha in lines:
            assert sha == shasum(p), (name, u, p, sha)
        assert code == (1 if bad else 0), (name, plan, code)
    return bad


FIXED = {
    "one red unit with a rerun beside a passing one": {"holon-gpui::main undo_windowed": ("RED", "RED"),
                                                       "holon-gpui::main chrome_windowed": ("PASS", "PASS")},
    "a green run": {"holon-gpui::main a_windowed": ("PASS", "PASS"), "holon-gpui::replay": ("KNOWN-RED", "PASS")},
    "a hung unit and a flaky one": {"holon-gpui::main a_windowed": ("HUNG", "HUNG"), "holon-gpui": ("RED", "PASS")},
}


def main():
    rng = random.Random(int(os.environ.get("SEED", 0)))
    for name, plan in FIXED.items():
        check(name, plan)
        print(f"[windowed-gate unit-log] {name}: PASS")
    cases, reds = int(os.environ.get("CASES", 40)), 0
    for case in range(cases):
        units = rng.sample(["holon-gpui", "holon-gpui::replay", *(f"holon-gpui::main m{i}_windowed" for i in range(5))],
                           rng.randint(1, 6))
        plan = {u: (rng.choice(FIRST), rng.choice(RERUN)) for u in units}
        reds += bool(check(f"case {case}", plan))
    assert 0 < reds < cases, f"{reds} of {cases} drawn runs left a unit RED or HUNG"
    print(f"[windowed-gate unit-log] {cases} drawn runs PASS ({reds} with a RED or HUNG unit)")


if __name__ == "__main__":
    main()
