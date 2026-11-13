#!/usr/bin/env python3
"""Property check of the `nextest:` line that scripts/select-tests.py prints, run as a lane runs it.

Run: `python3 scripts/test-select-tests-nextest-line.py` (CASES=<n> draws n change sets, default 4).
It builds the self-check's fixture workspace as real crates in a temp dir, prints the selection of each change set
there, and runs each printed `nextest:` line through scripts/flow-checks/run-step.sh, under a drawn nextest
default for an empty run (NEXTEST_NO_TESTS unset, `pass` or `warn`):
- the run exits non-zero exactly when the line matches no test (`cargo nextest list` of the same arguments);
- tests-cover-diff accepts the run-step log of a line that matches tests;
- `scripts/lane-nextest.py rerun` of the same arguments reruns one of its tests, as `just lane-nextest-reds` does
  (nextest refuses a second `--no-tests`);
- a line matches no test only when it names a crate that holds no test (TESTLESS): every other crate of the
  fixture holds a test, so an empty line is one whose crates the profile excludes;
- the crates of the line plus the crates logged as `<pkg>: no test of this profile` are the selected crates, and
  each dropped crate keeps its owner line (or the `unowned:` line naming it).
Then, over drawn default filters (`package()`, `test()` and `binary_id()` terms under `|`, `+`, `&`, `not`), every
crate that `excluded_packages` returns lists no test under that filter; the draws must prove some crate excluded.
Exits non-zero when a property fails; each failing change set is printed as an EXAMPLES entry.
"""
import contextlib
import importlib.util
import io
import json
import os
import random
import re
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
EXAMPLES = [(["crates/bare/src/lib.rs"], None), (["crates/bare/src/lib.rs"], "pass"), (["crates/core/src/lib.rs"], "pass"),
            (["crates/gpui/tests/window.rs"], None)]
TESTLESS = frozenset({"bare"})
# Draws of default filters: the soundness of `excluded_packages`, before the drawn ones.
FILTER_EXAMPLES = ["not (package(gpui) | package(leaf) & test(=lib))", "not (package(core)) & not (package(rows) + package(app))",
                   "not (package(core) | test(=w)) | package(core)", "not (not (package(core)))", "package(core)"]


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
        text = lib.read_text() if lib.exists() else ""
        lib.write_text(text if name in TESTLESS else text + "\n#[test] fn lib() {}\n")
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


def crates_of(line):
    return set(re.findall(r"\bpackage\(([\w-]+)\)", line))


def drop_problems(root, lines, log):
    """The selected crates (the stderr reason lines) are the crates of the nextest line plus the dropped ones, and
    each dropped crate keeps the line of its owner recipe."""
    selected = set(re.findall(r"^  (\S+): ", log, re.M))
    dropped = set(re.findall(r"^(\S+): no test of this profile$", log, re.M))
    line = next((line for line in lines if line.startswith("nextest: ")), "")
    found = []
    if crates_of(line) | dropped != selected:
        found.append(f"selected {sorted(selected)}, printed {sorted(crates_of(line))}, dropped {sorted(dropped)}")
    owners = st.owner_recipes(root)
    for pkg in sorted(dropped):
        term = f"package({pkg})"
        if term in owners and st.recipe_line(owners[term]) not in lines:
            found.append(f"dropped {pkg} without the owner line {st.recipe_line(owners[term])!r}: {lines}")
        if term not in owners and not any(line.startswith("unowned: ") and term in line for line in lines):
            found.append(f"dropped {pkg} without an `unowned:` line naming {term}: {lines}")
    return found, dropped


def problems(root, meta, env, changed, default, seen):
    log = io.StringIO()
    with contextlib.redirect_stderr(log):
        lines = st.selection(root, meta, changed)
    found, dropped = drop_problems(root, lines, log.getvalue())
    line = next((line for line in lines if line.startswith("nextest: ")), None)
    if line is None or (line, default) in seen:
        return found, dropped
    seen.add((line, default))
    env = {k: v for k, v in env.items() if k != "NEXTEST_NO_TESTS"} | ({"NEXTEST_NO_TESTS": default} if default else {})
    args = shlex.split(line.removeprefix("nextest: "))
    tests = len(listed := matched(root, env, args))
    run = subprocess.run([str(RUN_STEP), line], cwd=root, env=env, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT, text=True)
    print(f"{changed}, NEXTEST_NO_TESTS={default}: `{line}` matches {tests} test(s), run exit {run.returncode}")
    if not tests and not crates_of(line) & TESTLESS:
        found.append(f"`{line}` matches no test and names no crate without tests")
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
    return found, dropped


def draw_filter(rng, depth=0):
    """A default filter of `package()`, `test()` and `binary_id()` terms of the fixture; often `not (...) & not (...)`."""
    if depth == 0 and rng.random() < 0.5:
        return " & ".join(f"not ({draw_sum(rng)})" for _ in range(rng.randint(1, 3)))
    if depth == 3 or rng.random() < 0.4:
        return rng.choice([f"package({rng.choice(list(st.FIXTURE_PACKAGES))})", "test(=lib)", "test(=w)", "test(/^li/)",
                           "binary_id(ofmt::deep)", "binary_id(gpui::window)"])
    kind = rng.choice(["|", "+", "&", "not", "()"])
    if kind == "not":
        return f"not {draw_filter(rng, depth + 1)}"
    if kind == "()":
        return f"({draw_filter(rng, depth + 1)})"
    return f"{draw_filter(rng, depth + 1)} {kind} {draw_filter(rng, depth + 1)}"


def draw_sum(rng):
    return f" {rng.choice('|+')} ".join(draw_filter(rng, 1) for _ in range(rng.randint(1, 4)))


def excluded_problems(root, env, filt):
    """Every crate `excluded_packages` returns for the default filter `filt` lists no test under it."""
    config = root / ".config/nextest.toml"
    own = config.read_text()
    config.write_text(own + f'\n[profile.drawn]\ndefault-filter = "{filt}"\n')
    try:
        kept = {b.partition("::")[0] for b, _ in matched(root, env, ["--workspace", "--profile", "drawn"])}
    finally:
        config.write_text(own)
    excluded = st.excluded_packages(filt)
    return [f"`{filt}`: excluded_packages returns {sorted(excluded & kept)}, which list tests"] if excluded & kept else [], excluded


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
        seen, dropping = set(), 0
        for changed, default in EXAMPLES + drawn:
            found, dropped = problems(root, meta, env, changed, default, seen)
            dropping += bool(dropped)
            for f in found:
                print(f"FAIL: EXAMPLES entry ({changed}, {default!r}): {f}")
            failures += len(found)
        if not dropping:
            print("FAIL: no change set dropped a crate: the drop properties ran on nothing")
            failures += 1
        proved = 0
        filters = FILTER_EXAMPLES + [draw_filter(rng) for _ in range(int(os.environ.get("FILTERS", "12")))]
        for filt in filters:
            found, excluded = excluded_problems(root, env, filt)
            print(f"default filter `{filt}`: excluded_packages {sorted(excluded)}")
            proved += bool(excluded)
            for f in found:
                print(f"FAIL: FILTER_EXAMPLES entry {filt!r}: {f}")
            failures += len(found)
        if not proved:
            print("FAIL: no drawn default filter proved a crate excluded: the soundness property ran on nothing")
            failures += 1
    print(f"test-select-tests-nextest-line: {failures} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
