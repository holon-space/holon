#!/usr/bin/env python3
"""Self-check of the per-unit logs that scripts/windowed-gate.py writes.

Run: `python3 scripts/flow-checks/test-windowed-gate-logs.py`. Exits non-zero on the first violation.
main() runs on one unit with the cargo calls replaced: its nextest command is a shell stub that fails on the
first run and passes on the rerun, so the unit leaves a first-run log and a rerun log.
The header is read with the lane check's own functions (`header`, `started`).
Generators: none — one fixed log layout and one seeded stale file are the whole input space of these properties.
"""
import contextlib
import importlib.util
import io
import os
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
GATE = REPO / "scripts/windowed-gate.py"
CLASSIFIER = REPO / "scripts/keystone-known-reds.sh"
LANE_CHECK = Path.home() / ".claude/skills/orchestrator/scripts/lane-check.py"
BASE = "4f9afab597faeb1d0e31b3c382074c8aabb35002"
UNIT = "holon-gpui"
FAIL_ONCE = """if [ -e "$1/ran-once" ]; then echo 'test result: ok. 1 passed'; exit 0; fi
touch "$1/ran-once"
echo "thread 'a::b' panicked at frontends/gpui/tests/a.rs:1:1:"
echo 'a windowed self-check failure'
echo '        FAIL [   0.001s] (1/1) holon-gpui::main a::b'
exit 1"""


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


lane_check = load(LANE_CHECK, "lane_check")


def run_gate(log_dir, scratch, base):
    gate = load(GATE, "windowed_gate")
    gate.build = lambda features, d: []
    gate.inventory = lambda reuse: [(UNIT, f"binary_id(={UNIT})", 1)]
    gate.record_and_report = lambda results: None
    gate.nextest_args = lambda reuse, filt: ["sh", "-c", FAIL_ONCE, "sh", str(scratch)]
    sys.argv = ["windowed-gate.py", "--features", "x", "--log-dir", str(log_dir), "--unit", UNIT]
    saved = os.environ.pop("BASE", None)
    if base is not None:
        os.environ["BASE"] = base
    out = io.StringIO()
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = gate.main()
    finally:
        os.environ.pop("BASE", None)
        if saved is not None:
            os.environ["BASE"] = saved
    assert code == 0, f"one unit that passes its rerun is FLAKY, exit 0: {code}\n{out.getvalue()}"
    return out.getvalue()


def tree():
    return subprocess.run(["jj", "log", "-r", "@", "--no-graph", "-T", "commit_id"], cwd=REPO, check=True,
                          capture_output=True, text=True).stdout.strip()


def classified(log):
    """The classifier's exit and output, without the header lines it echoes from the log."""
    r = subprocess.run([str(CLASSIFIER), str(log)], cwd=REPO, capture_output=True, text=True)
    return r.returncode, [line for line in r.stdout.splitlines() if not line.startswith(("BASE: ", "TREE: "))]


def every_log_has_the_run_header(base, want_base):
    with tempfile.TemporaryDirectory() as d:
        log_dir, scratch = Path(d) / "logs", Path(d) / "scratch"
        scratch.mkdir()
        run_gate(log_dir, scratch, base)
        logs = sorted(log_dir.glob("*.log"))
        assert [p.name for p in logs] == [f"{UNIT}.log", f"{UNIT}.rerun.log"], logs
        for log in logs:
            text = log.read_text()
            assert lane_check.header(text, "BASE") == want_base, f"{log.name}: BASE {lane_check.header(text, 'BASE')}"
            assert lane_check.header(text, "TREE") == tree(), f"{log.name}: TREE {lane_check.header(text, 'TREE')}"
            start, problem = lane_check.started(text)
            assert problem is None, f"{log.name}: {problem}"
            assert start <= int(log.stat().st_mtime), (log.name, start)
        first = log_dir / f"{UNIT}.log"
        bare = scratch / first.name
        bare.write_text("".join(first.read_text().splitlines(keepends=True)[3:]))
        assert classified(first) == classified(bare), \
            f"the classifier reads a log with the header as the same log without it: {classified(first)}"


def main():
    every_log_has_the_run_header(BASE, BASE)
    print("[windowed-gate logs] every per-unit and rerun log starts with the BASE/TREE/STARTED header: PASS")
    every_log_has_the_run_header(None, "(unset)")
    print("[windowed-gate logs] an unset BASE reads `BASE: (unset)`: PASS")


main()
