#!/usr/bin/env python3
"""Properties of scripts/windowed-gate.py over drawn unit verdicts and `--unit` selections:
- the `unit-log:` lines: one per log of every unit it leaves RED or HUNG (first run and rerun), each with the hash
  `shasum -a 256` gives for that file, and none for any other unit;
- the failed ids in $GATE_IDS_FILE: one `failed <unit>` per unit left RED, and none at all when a unit is left HUNG;
- `--unit` runs exactly the units it names.

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


def run_gate(plan, log_dir, selected=(), ids_file=None):
    """Runs main() over `plan` ({unit: (first verdict, rerun verdict)}) with `--unit` per unit of `selected`;
    returns (exit, stdout, the units it ran)."""
    gate = load_gate()
    ran = set()
    gate.build = lambda features, d: []
    gate.inventory = lambda reuse: [(unit, f"binary_id(={unit})", 1) for unit in sorted(plan)]
    gate.record_and_report = lambda results: None

    def run_one(unit, filt, tests, reuse, d, timeout, suffix=""):
        log = d / (unit.replace("::", "__").replace(" ", "__") + suffix + ".log")
        log.write_text(f"{unit}{suffix}: {random.random()}\n")
        verdict = plan[unit][1 if suffix else 0]
        ran.add(unit)
        return {"binary": unit, "filter": filt, "tests": tests, "wall": 0.0, "verdict": verdict, "log": log,
                "orphans_killed": 0}

    gate.run_one = run_one
    out, err = io.StringIO(), io.StringIO()
    sys.argv = ["windowed-gate.py", "--features", "x", "--log-dir", str(log_dir), *(a for u in selected for a in ("--unit", u))]
    os.environ.pop("GATE_IDS_FILE", None)
    if ids_file:
        os.environ["GATE_IDS_FILE"] = str(ids_file)
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = gate.main()
    return code, out.getvalue() + err.getvalue(), ran


def shasum(path):
    return subprocess.run(["shasum", "-a", "256", str(path)], check=True, capture_output=True,
                          text=True).stdout.split()[0]


def check(name, plan, selected=()):
    with tempfile.TemporaryDirectory() as d:
        log_dir, ids_file = Path(d), Path(d) / "ids"
        ids_file.write_text("")
        code, out, ran = run_gate(plan, log_dir, selected, ids_file)
        assert ran == set(selected or plan), (name, selected, ran)
        plan = {u: v for u, v in plan.items() if u in ran}
        bad = {u for u, (first, rerun) in plan.items() if first in ("RED", "HUNG") and rerun in ("RED", "HUNG")}
        stem = {u: str(log_dir / u.replace("::", "__").replace(" ", "__")) for u in plan}
        want = {(u, f"{stem[u]}{suffix}.log") for u in bad for suffix in ("", ".rerun")}
        lines = [m.groups() for line in out.splitlines() if (m := UNIT_LOG.fullmatch(line))]
        assert {(u, p) for u, p, _ in lines} == want and len(lines) == len(want), (name, plan, lines, want)
        for u, p, sha in lines:
            assert sha == shasum(p), (name, u, p, sha)
        assert code == (1 if bad else 0), (name, plan, code)
        hung = any(plan[u][1] == "HUNG" for u in bad)
        want_ids = set() if hung else {f"failed {u}" for u in bad}
        got_ids = ids_file.read_text().splitlines()
        assert set(got_ids) == want_ids and len(got_ids) == len(want_ids), (name, plan, got_ids, want_ids)
    return bad


FIXED = {
    "one red unit with a rerun beside a passing one": {"holon-gpui::main undo_windowed": ("RED", "RED"),
                                                       "holon-gpui::main chrome_windowed": ("PASS", "PASS")},
    "a green run": {"holon-gpui::main a_windowed": ("PASS", "PASS"), "holon-gpui::replay": ("KNOWN-RED", "PASS")},
    "a hung unit and a flaky one": {"holon-gpui::main a_windowed": ("HUNG", "HUNG"), "holon-gpui": ("RED", "PASS")},
    "a red unit beside a hung one": {"holon-gpui::main a_windowed": ("RED", "HUNG"), "holon-gpui": ("RED", "RED")},
}


def main():
    rng = random.Random(int(os.environ.get("SEED", 0)))
    for name, plan in FIXED.items():
        check(name, plan)
        print(f"[windowed-gate unit-log] {name}: PASS")
    check("one unit of a main module selected", {"holon-gpui::main a_windowed": ("RED", "RED"), "holon-gpui": ("RED", "RED")},
          ["holon-gpui::main a_windowed"])
    # One fixed string: the unknown-unit refusal has no input space to draw.
    with tempfile.TemporaryDirectory() as d:
        code, out, ran = run_gate({"holon-gpui": ("PASS", "PASS")}, Path(d), ["holon-gpui::main gone"])
        assert (code, ran) == (2, set()) and "no such unit: holon-gpui::main gone" in out, (code, ran, out)
    print("[windowed-gate unit-log] an unknown --unit exits 2: PASS")
    cases, reds = int(os.environ.get("CASES", 40)), 0
    for case in range(cases):
        units = rng.sample(["holon-gpui", "holon-gpui::replay", *(f"holon-gpui::main m{i}_windowed" for i in range(5))],
                           rng.randint(1, 6))
        plan = {u: (rng.choice(FIRST), rng.choice(RERUN)) for u in units}
        selected = rng.sample(units, rng.randint(1, len(units))) if rng.random() < 0.5 else []
        reds += bool(check(f"case {case}", plan, selected))
    assert 0 < reds < cases, f"{reds} of {cases} drawn runs left a unit RED or HUNG"
    print(f"[windowed-gate unit-log] {cases} drawn runs PASS ({reds} with a RED or HUNG unit)")


if __name__ == "__main__":
    main()
