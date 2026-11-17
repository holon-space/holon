#!/usr/bin/env python3
"""Self-check for the log header that scripts/flow-checks/run-step.sh writes.

Run: `python3 scripts/flow-checks/test-run-step-header.py`. Exits non-zero on the first violation.
The header is read with the lane check's own functions (`header`, `started`, `same_time`).
fixtures/run-step-header.txt is a run-step log with the three header lines, `echo` as the step.
A `nextest` or `keystone-smoke` log header holds `HOLON_PBT_TIMING=<mode>` (the mode its step ran with); other steps hold none.
Generators: none — the input is one fixed header shape, so a fixed log and one live run assure it.
The `keystone-smoke` step's timing mode has two inputs (caller sets it or not), so two live runs cover it.
The test-thread cap has one input, FLOW_JOB_CORES, in three classes (unset, a positive integer, anything else),
so fixed values of each class cover it; nextest's own `--help` names the thread count it resolved.
The case budget has one input, a caller's HOLON_PBT_BUDGET (unset or set), and the lane shape sets it either way.
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
# run-step.sh reads its TREE: line from the jj repo it runs in.
RUN_TMP = tempfile.TemporaryDirectory()
RUN_DIR = Path(RUN_TMP.name)
subprocess.run(["jj", "git", "init", "--quiet"], cwd=RUN_DIR, check=True)

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
    out = subprocess.run([str(RUN_STEP), "echo: echo hello"], cwd=RUN_DIR, capture_output=True, text=True,
                         env={**os.environ, "BASE": BASE}, check=True).stdout
    lines = out.splitlines()
    assert lines[:2] == ["echo: echo hello", f"BASE: {BASE}"], lines[:2]
    assert lines[2].startswith("TREE: ") and lines[3].startswith("STARTED: "), lines[:4]
    return out


def keystone_smoke_timing(caller):
    env = {k: v for k, v in os.environ.items() if k != "HOLON_PBT_TIMING"}
    out = subprocess.run([str(RUN_STEP), "keystone-smoke: printenv HOLON_PBT_TIMING"], cwd=RUN_DIR,
                         capture_output=True, text=True, env={**env, **caller, "BASE": BASE}).stdout
    return out.splitlines()[-3]


def check_timing_header():
    for step in ("nextest", "keystone-smoke"):
        for caller, want in (({}, "measure"), ({"HOLON_PBT_TIMING": "enforce"}, "enforce"),
                             ({"HOLON_PBT_TIMING": ""}, ""), ({"HOLON_PBT_TIMING": "measure"}, "measure")):
            env = {k: v for k, v in os.environ.items() if k != "HOLON_PBT_TIMING"}
            out = subprocess.run([str(RUN_STEP), f"{step}: printenv HOLON_PBT_TIMING"], cwd=RUN_DIR, capture_output=True,
                                 text=True, env={**env, **caller, "BASE": BASE}).stdout.splitlines()
            assert out[3].startswith("STARTED: ") and out[4] == f"HOLON_PBT_TIMING={want}", (step, caller, out[:6])
            assert step == "nextest" or out[-3] == want, (step, caller, "the header names the mode the step ran with", out[-4:])
    with tempfile.TemporaryDirectory() as stubs:
        cargo = Path(stubs) / "cargo"
        cargo.write_text('#!/bin/sh\necho "cargo-stub timing=$HOLON_PBT_TIMING"\n')
        cargo.chmod(0o755)
        for caller, want in (({}, "measure"), ({"HOLON_PBT_TIMING": "enforce"}, "enforce")):
            base_env = {k: v for k, v in os.environ.items() if k != "HOLON_PBT_TIMING"}
            run_env = {**base_env, **caller, "BASE": BASE, "PATH": f"{stubs}:{os.environ['PATH']}"}
            out = subprocess.run([str(RUN_STEP), "nextest: --no-tests=fail"], cwd=RUN_DIR, capture_output=True,
                                 text=True, env=run_env).stdout.splitlines()
            assert f"cargo-stub timing={want}" in out and f"HOLON_PBT_TIMING={want}" in out, (caller, out[-8:])
    out = live_log().splitlines()
    assert not any(l.startswith("HOLON_PBT_TIMING=") for l in out), "other steps carry no mode line"


CAP_VARS = ("FLOW_JOB_CORES", "NEXTEST_TEST_THREADS", "RUST_TEST_THREADS")
CAP_ERROR = "FLOW_JOB_CORES must be a positive integer"
SHOW_CAP = """python3 -c 'import os; print("threads:", *(os.environ.get(v) for v in ("NEXTEST_TEST_THREADS", "RUST_TEST_THREADS")))'"""


def run_step(line, cores, record):
    """run-step.sh `line` under a slot's NEXTEST_TEST_THREADS=8 and FLOW_JOB_CORES=`cores` (None: unset)."""
    env = {k: v for k, v in os.environ.items() if k not in CAP_VARS}
    env |= {"BASE": BASE, "NEXTEST_TEST_THREADS": "8", "HOLON_CARGO_RECORD": record}
    if cores is not None:
        env["FLOW_JOB_CORES"] = cores
    return subprocess.run([str(RUN_STEP), line], cwd=RUN_DIR, capture_output=True, text=True, env=env)


def threads(step, cores, record):
    p = run_step(f"{step}: {SHOW_CAP}", cores, record)
    assert p.returncode == 0, (step, cores, p.stderr)
    lines = p.stdout.splitlines()
    return next(l for l in lines if l.startswith("threads: ")), f"shape env: NEXTEST_TEST_THREADS={cores or 8}" in lines


def check_test_thread_cap(record):
    for cores in ("1", "2", "16"):
        for step in ("echo", "keystone-smoke"):
            got, header = threads(step, cores, record)
            assert got == f"threads: {cores} {cores}", (step, cores, got)
            assert header, f"the log header names the cap {cores}"
    p = run_step("nextest: --no-tests=fail --help", "2", record)
    assert "[env: NEXTEST_TEST_THREADS=2]" in p.stdout, f"nextest resolves the cap: {p.stdout[-2000:]}"
    assert threads("echo", None, record) == ("threads: 8 None", True), "no job: the slot's thread count, no cargo test cap"
    for bad in ("", "0", "07", "x", "-2", "2.5", " 2"):
        p = run_step("echo: echo ran", bad, record)
        assert p.returncode == 1 and "ran" not in p.stdout.splitlines(), (bad, p.stdout)
        assert f"{CAP_ERROR}, got '{bad}'" in p.stderr, (bad, p.stderr)


def check_lane_budget(record):
    for caller in (None, "gate"):
        env = {k: v for k, v in os.environ.items() if k != "HOLON_PBT_BUDGET"}
        env |= {"BASE": BASE, "HOLON_CARGO_RECORD": record}
        if caller is not None:
            env["HOLON_PBT_BUDGET"] = caller
        p = subprocess.run([str(RUN_STEP), "echo: printenv HOLON_PBT_BUDGET"], cwd=HERE, capture_output=True,
                           text=True, env=env)
        lines = p.stdout.splitlines()
        assert p.returncode == 0 and lines[-3] == "lane", (caller, p.stdout[-500:], p.stderr[-500:])
        assert "shape env: HOLON_PBT_BUDGET=lane" in lines, "the log header names the lane budget"


def main():
    with tempfile.TemporaryDirectory() as d:
        check_test_thread_cap(os.path.join(d, "calls.jsonl"))
        check_lane_budget(os.path.join(d, "calls.jsonl"))
    check_timing_header()
    assert keystone_smoke_timing({}) == "measure", "a keystone-smoke step runs in measure mode by default"
    assert keystone_smoke_timing({"HOLON_PBT_TIMING": "enforce"}) == "enforce", "the caller's mode wins"
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
