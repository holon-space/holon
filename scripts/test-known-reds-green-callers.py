#!/usr/bin/env python3
"""Check that every gate caller of scripts/keystone-known-reds.sh reports a stale known-red row from a green step.

Run: `python3 scripts/test-known-reds-green-callers.py`. Stub `cargo` and `just` print a green nextest log; the callers
run the real classifier on it. Per caller step, a log without a stale row exits 0 with no `[known-reds]` output, a log with a
PASS of a test of the registry's `Test | Registry key` table exits 0 with the STALE report, and a broken table fails
the step. Fixed examples, not a property check: the inputs are these three log classes.
"""
import contextlib
import importlib.util
import io
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import known_reds_logs as logs

REPO = Path(__file__).resolve().parent.parent
STUB = '#!/bin/sh\nfor a; do [ "$a" != --evaluate ] || { echo stub; exit 0; }; done\ncat "$STUB_LOG"\n'
TABLE_HEADER = "| Test | Registry key |\n| --- | --- |\n"
# These find the classifier by their resolved path, so the tree holds copies, not links.
RESOLVING = {"lane-nextest.py", "windowed-gate.py"}


def checkout(root, broken):
    """A tree at `root` that shares the repository's scripts; with `broken`, its table names a row that does not exist."""
    (root / "docs/Testing").mkdir(parents=True)
    (root / "target/gate-logs").mkdir(parents=True)
    (root / "scripts").mkdir()
    for f in (REPO / "scripts").iterdir():
        if f.name in RESOLVING:
            (root / "scripts" / f.name).write_text(f.read_text())
        else:
            (root / "scripts" / f.name).symlink_to(f)
    (root / "justfile").symlink_to(REPO / "justfile")
    (root / "docs/Testing/known-red-evidence").symlink_to(REPO / "docs/Testing/known-red-evidence")
    registry = (REPO / "docs/Testing/KeystoneKnownReds.md").read_text()
    assert registry.count(TABLE_HEADER) == 1
    row = "| `holon-x::main a::case_b` | `no-such-row` |\n" if broken else ""
    (root / "docs/Testing/KeystoneKnownReds.md").write_text(registry.replace(TABLE_HEADER, TABLE_HEADER + row))


def gate_step(step):
    def run(root, env):
        out = subprocess.run(["bash", "scripts/gate-integrate-steps.sh", step], cwd=root, capture_output=True, text=True,
                             env={**env, "GATE": "fixture", "PBT_FEATURES": "stub"})
        return out.returncode, out.stdout, out.stderr
    return run


def lane_nextest(root, env):
    out = subprocess.run([sys.executable, str(root / "scripts/lane-nextest.py"), "run", "--no-tests=fail"], cwd=root, capture_output=True,
                         text=True, env={**env, "LANE_NEXTEST_DIR": str(root / "target/lane-nextest")})
    return out.returncode, out.stdout, out.stderr


def windowed(root, env):
    spec = importlib.util.spec_from_file_location("windowed_gate", root / "scripts/windowed-gate.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.RUN.update(base="stub", tree="stub")
    printed = io.StringIO()
    saved = dict(os.environ)
    os.environ.update(env)
    try:
        with contextlib.redirect_stdout(printed):
            verdict = module.run_one("holon-x::main", "all()", 2, [], root / "target", 60)["verdict"]
        code = 0 if verdict == "PASS" else 1
    except subprocess.CalledProcessError as e:
        code = e.returncode
    finally:
        os.environ.clear()
        os.environ.update(saved)
    return code, printed.getvalue(), ""


CALLERS = [("gate keystone-smoke", gate_step("keystone-smoke")), ("gate hand-authored", gate_step("hand-authored")),
           ("gate extra-nextest", gate_step("extra-nextest")), ("gate keystone-tour", gate_step("keystone-tour")),
           ("lane-nextest", lane_nextest),
           ("windowed-gate", windowed)]


def main():
    pinned, key = logs.pinned_test()
    stale = f"[known-reds] STALE: {key}: its test {pinned} passed at "
    other = "holon-x::main a::case_b"
    cases = [("no stale row", (other, "holon-x::main a::case_c"), False,
              lambda code, out: code == 0 and "[known-reds]" not in out),
             ("a stale row", (other, pinned), False, lambda code, out: code == 0 and stale in out),
             ("a broken table", (other, "holon-x::main a::case_c"), True, lambda code, out: code != 0)]
    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        (tmp / "bin").mkdir()
        for tool in ("cargo", "just"):
            (tmp / "bin" / tool).write_text(STUB)
            (tmp / "bin" / tool).chmod(0o755)
        for n, (case, units, broken, ok) in enumerate(cases):
            log = tmp / f"case-{n}.log"
            log.write_text(logs.green_run(*units))
            # The fixture's cargo calls stay out of the caller's record: a gate checks that record's compile shape.
            env = {**os.environ, "PATH": f"{tmp / 'bin'}:{os.environ['PATH']}", "STUB_LOG": str(log),
                   "HOLON_CARGO_RECORD": str(tmp / "cargo-calls.jsonl")}
            for m, (caller, run) in enumerate(CALLERS):
                root = tmp / f"tree-{n}-{m}"
                checkout(root, broken)
                code, out, err = run(root, env)
                good = ok(code, out)
                failed += not good
                print(f"{'ok' if good else 'FAIL'} {caller}, {case}: exit {code}" + ("" if good else f"\n{out}{err}"))
    print(f"[known-reds-green-callers] {'FAIL' if failed else 'PASS'}: {failed} failing case(s)")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
