#!/usr/bin/env python3
"""Self-check of the per-unit logs that scripts/windowed-gate.py writes.

Run: `python3 scripts/flow-checks/test-windowed-gate-logs.py` (CASES=<n>, SEED=<s>). Exits non-zero on the first violation.
main() runs on one unit with the cargo calls replaced: its nextest command is a shell stub that fails on the
first run and passes on the rerun, so the unit leaves a first-run log and a rerun log.
The header is read with the lane check's own functions (`header`, `started`).
Generators: the tree id (40 lowercase hex), BASE (unset or a hex id) and a malformed `--tree` value, drawn from SEED; the
fixed examples run first through the same properties. The log layout and the seeded stale file are fixed.
Properties: every log header carries the drawn tree id and BASE, also from a copy of the gate inside a jj repo whose `@`
differs and with a failing `jj` first on PATH (the gate asks no VCS); a missing or malformed `--tree` exits 2 and leaves
the log dir as it was.
"""
import contextlib
import importlib.util
import io
import os
import random
import shutil
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


def run_gate(log_dir, scratch, base, tree, gate_path=GATE):
    gate = load(gate_path, "windowed_gate")
    gate.CLASSIFIER = CLASSIFIER
    gate.build = lambda features, d: []
    gate.inventory = lambda reuse: [(UNIT, f"binary_id(={UNIT})", 1)]
    gate.record_and_report = lambda results: None
    gate.nextest_args = lambda reuse, filt: ["sh", "-c", FAIL_ONCE, "sh", str(scratch)]
    sys.argv = ["windowed-gate.py", "--features", "x", "--log-dir", str(log_dir), "--unit", UNIT, "--tree", tree]
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


def hex40(rng):
    return "".join(rng.choice("0123456789abcdef") for _ in range(40))


def malformed(rng):
    good = hex40(rng)
    return rng.choice(["", "@", "x" * 40, good[:39], good + "0", good.upper(), f" {good}", f"{good}\n", "-" + good[1:]])


def classified(log):
    """The classifier's exit and output, without the header lines it echoes from the log."""
    r = subprocess.run([str(CLASSIFIER), str(log)], cwd=REPO, capture_output=True, text=True)
    return r.returncode, [line for line in r.stdout.splitlines() if not line.startswith(("BASE: ", "TREE: "))]


def every_log_has_the_run_header(base, want_base, tree, gate_path=GATE):
    with tempfile.TemporaryDirectory() as d:
        log_dir, scratch = Path(d) / "logs", Path(d) / "scratch"
        scratch.mkdir()
        run_gate(log_dir, scratch, base, tree, gate_path)
        logs = sorted(log_dir.glob("*.log"))
        assert [p.name for p in logs] == [f"{UNIT}.log", f"{UNIT}.rerun.log"], logs
        for log in logs:
            text = log.read_text()
            assert lane_check.header(text, "BASE") == want_base, f"{log.name}: BASE {lane_check.header(text, 'BASE')}"
            assert lane_check.header(text, "TREE") == tree, f"{log.name}: TREE {lane_check.header(text, 'TREE')}"
            start, problem = lane_check.started(text)
            assert problem is None, f"{log.name}: {problem}"
            assert start <= int(log.stat().st_mtime), (log.name, start)
        first = log_dir / f"{UNIT}.log"
        bare = scratch / first.name
        bare.write_text("".join(first.read_text().splitlines(keepends=True)[3:]))
        assert classified(first) == classified(bare), \
            f"the classifier reads a log with the header as the same log without it: {classified(first)}"


def only_this_runs_logs_stay(tree):
    with tempfile.TemporaryDirectory() as d:
        log_dir, scratch = Path(d) / "logs", Path(d) / "scratch"
        scratch.mkdir()
        log_dir.mkdir()
        (log_dir / "stale.rerun.log").write_text("thread 'old::run' panicked at a.rs:1:1:\nan older run\n")
        (log_dir / "stale.rerun.classified").write_text("PRIMARY: [novel] stale.rerun.log\n")
        (log_dir / "notes.txt").write_text("not a log of the gate\n")
        run_gate(log_dir, scratch, BASE, tree)
        names = sorted(p.name for p in log_dir.iterdir())
        assert names == ["holon-gpui.classified", "holon-gpui.log", "holon-gpui.rerun.log", "notes.txt"], names


def in_a_foreign_repo(base, want_base, tree):
    """The gate copy sits in a jj repo whose `@` is not `tree`."""
    with tempfile.TemporaryDirectory() as d:
        root = Path(d) / "repo"
        (root / "scripts").mkdir(parents=True)
        subprocess.run(["jj", "git", "init"], cwd=root, check=True, capture_output=True)
        shutil.copy(GATE, root / "scripts/windowed-gate.py")
        foreign = subprocess.run(["jj", "log", "-r", "@", "--no-graph", "-T", "commit_id"], cwd=root, check=True,
                                 capture_output=True, text=True).stdout.strip()
        assert foreign != tree, (foreign, tree)
        every_log_has_the_run_header(base, want_base, tree, root / "scripts/windowed-gate.py")


def with_a_failing_jj(base, want_base, tree):
    with tempfile.TemporaryDirectory() as d:
        stub = Path(d) / "jj"
        stub.write_text("#!/bin/sh\necho 'jj must not run' >&2\nexit 1\n")
        stub.chmod(0o755)
        saved = os.environ["PATH"]
        os.environ["PATH"] = f"{d}:{saved}"
        try:
            every_log_has_the_run_header(base, want_base, tree)
        finally:
            os.environ["PATH"] = saved


def refuses_a_bad_tree(value):
    """`value` None leaves `--tree` out. Exit 2 from argparse, and the seeded logs stay."""
    with tempfile.TemporaryDirectory() as d:
        log_dir = Path(d) / "logs"
        log_dir.mkdir()
        stale = log_dir / "stale.log"
        stale.write_text("an older run\n")
        argv = [sys.executable, str(GATE), "--features", "x", "--log-dir", str(log_dir)]
        r = subprocess.run(argv + ([] if value is None else ["--tree", value]), capture_output=True, text=True)
        assert r.returncode == 2, (value, r.returncode, r.stderr)
        assert "--tree" in r.stderr, (value, r.stderr)
        assert [p.name for p in log_dir.iterdir()] == ["stale.log"] and stale.read_text() == "an older run\n", value


def main():
    rng = random.Random(int(os.environ.get("SEED", 0)))
    fixed = "0123456789abcdef0123456789abcdef01234567"
    draws = [(BASE, BASE, fixed), (None, "(unset)", fixed)]
    for _ in range(int(os.environ.get("CASES", 6))):
        base = rng.choice([None, hex40(rng)])
        draws.append((base, base or "(unset)", hex40(rng)))
    for base, want_base, tree in draws:
        every_log_has_the_run_header(base, want_base, tree)
        with_a_failing_jj(base, want_base, tree)
    print("[windowed-gate logs] every per-unit and rerun log starts with the BASE/TREE/STARTED header, TREE the --tree id, "
          "BASE `(unset)` when unset, with a failing jj on PATH: PASS")
    for base, want_base, tree in draws[:3]:
        in_a_foreign_repo(base, want_base, tree)
    print("[windowed-gate logs] a gate copy inside a jj repo whose @ differs still names the --tree id: PASS")
    only_this_runs_logs_stay(fixed)
    print("[windowed-gate logs] a stale log and classifier file of an older run are gone, other files stay: PASS")
    for value in [None, "", "@", "x" * 40, fixed[:39], fixed + "0", fixed.upper(), *(malformed(rng) for _ in range(6))]:
        refuses_a_bad_tree(value)
    print("[windowed-gate logs] a missing or malformed --tree exits 2 and leaves the log dir as it was: PASS")


main()
