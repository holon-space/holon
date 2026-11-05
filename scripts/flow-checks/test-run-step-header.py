#!/usr/bin/env python3
"""Self-check for the log header that scripts/flow-checks/run-step.sh writes.

Run: `python3 scripts/flow-checks/test-run-step-header.py`. Exits non-zero on the first violation.
The header is read with the lane check's own functions (`header`, `started`, `same_time`).
fixtures/run-step-header.txt is a run-step log with the three header lines, `echo` as the step.
Generators: none — the input is one fixed header shape, so a fixed log and one live run assure it.
"""
import importlib.util
import os
import subprocess
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUN_STEP = HERE / "run-step.sh"
FIXTURE = HERE / "fixtures/run-step-header.txt"
LANE_CHECK = Path.home() / ".claude/skills/orchestrator/scripts/lane-check.py"
BASE = "4f9afab597faeb1d0e31b3c382074c8aabb35002"

spec = importlib.util.spec_from_file_location("lane_check", LANE_CHECK)
lane_check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lane_check)


def assert_l1_header(text, label):
    assert lane_check.header(text, "BASE"), f"{label}: no BASE: line"
    assert lane_check.header(text, "TREE"), f"{label}: no TREE: line"
    start, problem = lane_check.started(text)
    assert problem is None, f"{label}: {problem}"
    return start


def live_log():
    out = subprocess.run([str(RUN_STEP), "echo: echo hello"], cwd=HERE, capture_output=True, text=True,
                         env={**os.environ, "BASE": BASE}, check=True).stdout
    lines = out.splitlines()
    assert lines[:2] == ["echo: echo hello", f"BASE: {BASE}"], lines[:2]
    assert lines[2].startswith("TREE: ") and lines[3].startswith("STARTED: "), lines[:4]
    return out


def main():
    assert_l1_header(FIXTURE.read_text(), "fixture")
    text = live_log()
    start = assert_l1_header(text, "live log")
    with tempfile.NamedTemporaryFile("w") as f:
        f.write(text)
        f.flush()
        run = (start, int(os.stat(f.name).st_mtime))
    assert start <= run[1], run
    assert lane_check.same_time({"echo.log": run}) == []
    print("ok")


main()
