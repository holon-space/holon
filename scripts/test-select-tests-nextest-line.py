#!/usr/bin/env python3
"""Property check of the `nextest:` line that scripts/select-tests.py prints, run as a lane runs it.

Run: `python3 scripts/test-select-tests-nextest-line.py` (CASES=<n> draws n change sets, default 4).
It builds the self-check's fixture workspace as real crates in a temp dir, prints the selection of each change set
there, and runs each printed `nextest:` line through scripts/flow-checks/run-step.sh, under a drawn nextest
default for an empty run (NEXTEST_NO_TESTS unset, `pass` or `warn`):
- the run exits non-zero exactly when the line matches no test (`cargo nextest list` of the same arguments);
- tests-cover-diff accepts the run-step log of a line that matches tests;
- `scripts/lane-nextest.py rerun` of the same arguments reruns one of its tests, as `just lane-nextest-reds` does
  (nextest refuses a second `--no-tests`).
Exits non-zero when a property fails; each failing change set is printed as an EXAMPLES entry.
"""
import contextlib
import importlib.util
import io
import json
import os
import random
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RUN_STEP = REPO / "scripts/flow-checks/run-step.sh"
FLAG = "--no-tests=fail"
DEFAULTS = (None, "pass", "warn")
# (change set, NEXTEST_NO_TESTS), run before the drawn ones: selections that match no test and one that matches tests.
EXAMPLES = [(["crates/leaf/src/lib.rs"], None), (["crates/leaf/src/lib.rs"], "pass"), (["crates/core/src/lib.rs"], "pass")]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


st = load("select_tests", REPO / "scripts/select-tests.py")
tcd = load("tests_cover_diff", REPO / "scripts/flow-checks/tests-cover-diff.py")
# Every changed path of the self-check's golden cases that has a selection; the drawn change sets combine them.
PATHS = sorted({p for changed, want, _ in st.GOLDEN if want is not ValueError for p in changed})


def manifest(name, deps, tests):
    lines = [f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\nautotests = false\n',
             "[dependencies]"]
    lines += [f'{d} = {{ path = "../{d}" }}' for d in deps]
    lines.append("\n[features]\npbt = []")
    lines += [f'\n[[test]]\nname = "{t}"\npath = "{src}"' for t, src in tests.items()]
    return "\n".join(lines) + "\n"


def real_workspace(root):
    """The self-check's fixture workspace with real manifests and sources; returns its `cargo metadata`."""
    st.fixture_workspace(root)
    members = []
    for name, (deps, tests) in st.FIXTURE_PACKAGES.items():
        crate = root / "crates" / name
        (crate / "Cargo.toml").write_text(manifest(name, deps, tests))
        lib = crate / "src/lib.rs"
        lib.parent.mkdir(parents=True, exist_ok=True)
        if not lib.exists():
            lib.write_text("")
        members.append(f'"crates/{name}"')
    (root / "Cargo.toml").write_text(f'[workspace]\nresolver = "2"\nmembers = [{", ".join(members)}]\n')
    out = subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=root, check=True,
                         stdout=subprocess.PIPE, text=True)
    return json.loads(out.stdout)


def matched(root, env, args):
    """[(binary id, test name)] that `cargo nextest list` runs for the nextest `args`, the profile's default filter
    applied."""
    out = subprocess.run(["cargo", "nextest", "list", "--message-format", "json",
                          *[a for a in args if a != FLAG]], cwd=root, env=env, check=True,
                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    suites = json.loads(out.stdout)["rust-suites"].items()
    return [(b, name) for b, s in suites for name, case in s["testcases"].items()
            if case["filter-match"]["status"] == "matches"]


def nextest_line(root, meta, changed):
    with contextlib.redirect_stderr(io.StringIO()):
        lines = st.selection(root, meta, changed)
    return next((line for line in lines if line.startswith("nextest: ")), None)


def problems(root, meta, env, changed, default, seen):
    line = nextest_line(root, meta, changed)
    if line is None or (line, default) in seen:
        return [], line
    seen.add((line, default))
    env = {k: v for k, v in env.items() if k != "NEXTEST_NO_TESTS"} | ({"NEXTEST_NO_TESTS": default} if default else {})
    args = shlex.split(line.removeprefix("nextest: "))
    tests = len(listed := matched(root, env, args))
    run = subprocess.run([str(RUN_STEP), line], cwd=root, env=env, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT, text=True)
    print(f"{changed}, NEXTEST_NO_TESTS={default}: `{line}` matches {tests} test(s), run exit {run.returncode}")
    found = []
    if (run.returncode != 0) != (tests == 0):
        found.append(f"`{line}` matches {tests} test(s) and the run exits {run.returncode}")
    if tests:
        summary = f"How tested:\n- `{line}` -> target/lane/nextest.log\nConfidence (1-5): 5\n"
        bad = tcd.check(line + "\n", summary, {"target/lane/nextest.log": run.stdout}, set(st.NOT_RUN_STEPS))
        found += [f"tests-cover-diff on the log of `{line}`: {b}" for b in bad]
        reds = root / "target/lane-nextest"
        reds.mkdir(parents=True, exist_ok=True)
        (reds / "reds.txt").write_text(" ".join(listed[0]) + "\n")
        rerun = subprocess.run([str(REPO / "scripts/lane-nextest.py"), "rerun", *args], cwd=root,
                               env=env | {"LANE_NEXTEST_DIR": str(reds)}, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True)
        if rerun.returncode:
            found.append(f"the rerun of `{line}` exits {rerun.returncode}: {rerun.stdout.strip()[-300:]}")
    return found, line


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**32)))
    rng = random.Random(seed)
    drawn = [(rng.sample(PATHS, rng.randint(1, 3)), rng.choice(DEFAULTS))
             for _ in range(int(os.environ.get("CASES", "4")))]
    print(f"drawn with seed {seed}: {drawn}")
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        meta = real_workspace(root)
        env = {**os.environ, "BASE": "0" * 40, "CARGO_TARGET_DIR": str(root / "target")}
        seen = set()
        for changed, default in EXAMPLES + drawn:
            found, _ = problems(root, meta, env, changed, default, seen)
            for f in found:
                print(f"FAIL: EXAMPLES entry ({changed}, {default!r}): {f}")
            failures += len(found)
    print(f"test-select-tests-nextest-line: {failures} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
