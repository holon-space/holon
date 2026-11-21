#!/usr/bin/env python3
"""`just gate-arch` runs every check, prints one verdict line per check and fails iff a check failed.

Run: `python3 scripts/test-gate-arch-verdicts.py`.
Each example runs the real recipe text in a scratch folder where every check is a stub that prints one line and
exits with the example's code, and asserts:
- one `PASS <check>` or `FAIL <check> (exit N)` line per check, in recipe order, N the stub's own exit;
- the recipe exits non-zero iff a check failed;
- each piped check's log holds the check's output.
An example list, not drawn cases: the input is which checks are red, and the list holds one example per class
(none, the first, two, the last, one in between).
"""
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
# (check name, what the stub replaces, its log under target/gate-logs or None), in recipe order.
CHECKS = [
    ("gate-arch-verdicts", "scripts/test-gate-arch-verdicts.py", "gate-arch-verdicts.log"),
    ("run-step-header", "scripts/flow-checks/test-run-step-header.py", "gate-arch-run-step-header.log"),
    ("select-tests-registry-data", "scripts/test-select-tests-registry-data.py",
     "gate-arch-select-tests-registry-data.log"),
    ("select-tests-dev-edges", "scripts/test-select-tests-dev-edges.py", "gate-arch-select-tests-dev-edges.log"),
    ("light-paths-gate", "scripts/test-light-paths-gate.py", "gate-arch-light-paths-gate.log"),
    ("gate-integrate-tmpdir", "scripts/test-gate-integrate-tmpdir.py", "gate-arch-gate-integrate-tmpdir.log"),
    ("windowed-gate-logs", "scripts/flow-checks/test-windowed-gate-logs.py", "gate-arch-windowed-gate-logs.log"),
    ("windowed-gate-unit-logs", "scripts/test-windowed-gate-unit-logs.py", "gate-arch-windowed-gate-unit-logs.log"),
    ("architecture_rules", "cargo", "gate-arch.log"),
]
NAMES = [name for name, _, _ in CHECKS]
EXAMPLES = [
    set(),
    {"gate-arch-verdicts"},
    {"run-step-header", "windowed-gate-logs", "windowed-gate-unit-logs"},
    {"architecture_rules"},
    {"gate-integrate-tmpdir"},
]
VERDICT = re.compile(r"(PASS|FAIL) .*")


def stub(name, code):
    return f'#!/bin/sh\necho "stub {name} ran"\nexit {code}\n'


def run(scratch, recipe, red):
    exits = {name: 3 + i if name in red else 0 for i, name in enumerate(NAMES)}
    (scratch / "stubs").mkdir(exist_ok=True)
    for name, replaces, _ in CHECKS:
        path = scratch / ("stubs/cargo" if replaces == "cargo" else replaces)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(stub(name, exits[name]))
        path.chmod(0o755)
    (scratch / "justfile").write_text(f'CANON := "--stubbed"\n\n{recipe}')
    logs = scratch / "target/gate-logs"
    for log in logs.glob("*.log") if logs.exists() else []:
        log.unlink()
    env = {**os.environ, "PATH": f"{scratch / 'stubs'}:{os.environ['PATH']}"}
    r = subprocess.run(["just", "gate-arch"], cwd=scratch, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                       text=True)
    return r.returncode, r.stdout, exits


def problems(rc, out, exits, scratch):
    want = [f"FAIL {n} (exit {exits[n]})" if exits[n] else f"PASS {n}" for n in NAMES]
    got = [line for line in out.splitlines() if VERDICT.fullmatch(line)]
    bad = []
    if got != want:
        bad.append(f"verdict lines {got}, want {want}")
    if (rc != 0) != any(exits.values()):
        bad.append(f"exit {rc} with red checks {[n for n in NAMES if exits[n]]}")
    for name, _, log in CHECKS:
        path = scratch / "target/gate-logs" / log if log else None
        if path and f"stub {name} ran" not in (path.read_text() if path.exists() else ""):
            bad.append(f"log {log} lacks the output of {name}")
    return bad


def main():
    recipe = subprocess.run(["just", "--show", "gate-arch"], cwd=REPO, check=True, capture_output=True,
                            text=True).stdout
    failures = []
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        for red in EXAMPLES:
            rc, out, exits = run(scratch, recipe, red)
            bad = problems(rc, out, exits, scratch)
            print(f"red {sorted(red)}: exit {rc}, {'PASS' if not bad else 'FAIL'}")
            failures += [f"EXAMPLES entry {sorted(red)}: {b}\n" + "\n".join("  | " + line for line in
                                                                           out.splitlines()[-20:]) for b in bad]
    for failure in failures:
        print(f"FAIL: {failure}")
    print(f"test-gate-arch-verdicts: {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
