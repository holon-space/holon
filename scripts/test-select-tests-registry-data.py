#!/usr/bin/env python3
"""Property check of the lane selection that scripts/select-tests.py prints for a diff of registry data.

Run from the workspace root: `python3 scripts/test-select-tests-registry-data.py` (SEED=<n>, CASES=<n>, default 40;
SELECT_TESTS_BASE=<jj rev> also compares with the selector of that revision).
Registry data is the known-red registry, its evidence dir and the generated FeatureMap. Each change set is drawn
from the tracked files of this tree plus deleted evidence paths and near-miss paths, and its input class decides
the oracle:
- registry only, or registry plus other registry data: the printed lines are exactly the registry's own checks and
  stderr names the rule (P1; P1 also makes adding or removing data paths beside the registry change nothing, P3);
- every other class (data without the registry, any non-data path): the lines equal the selection with the rule
  switched off (P2) and, with SELECT_TESTS_BASE, the selection of that revision; for data without the registry,
  adding a data path keeps both equalities (P3);
- the lines do not depend on the order of the changed paths;
- the integrate gate keeps the tour and the hand-authored cases for registry data.
Fixed cases run first. Each failing case is printed as its explicit path list.
"""
import contextlib
import importlib.util
import inspect
import io
import os
import random
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path, PurePosixPath

REPO = Path(__file__).resolve().parent.parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


st = load("select_tests", REPO / "scripts/select-tests.py")
evidence = load("check_known_red_evidence", REPO / "scripts/check-known-red-evidence.py")

REGISTRY = "docs/Testing/KeystoneKnownReds.md"
FEATUREMAP = "docs/Architecture/FeatureMap.md"
EVIDENCE_DIR = "docs/Testing/known-red-evidence/"
GONE_EVIDENCE = "docs/Testing/known-red-evidence/a-retired-row/keystone-smoke-gate.log.zst"
# Paths beside registry data that are not registry data.
NEAR_MISS = ["docs/Testing/WindowedKnownReds.md", "docs/Architecture/featuremap.yaml",
             "docs/Testing/known-red-evidence-x/a.log", "docs/Testing/KeystoneKnownReds.md.orig",
             "docs/Testing/known-red-evidence.md"]
ROWS_ONLY_DIFF = [REGISTRY, FEATUREMAP,
                  "docs/Testing/known-red-evidence/typechars-no-focused-block/keystone-smoke-gate.log.zst",
                  "docs/Testing/known-red-evidence/splitblock-caret-past-dense-projection-content/keystone-smoke-gate.log.zst"]
REGISTRY_ONLY, REGISTRY_AND_DATA, DATA_NO_REGISTRY, MIXED = ("registry only", "registry + other data",
                                                            "data without registry", "mixed")
CLASSES = (REGISTRY_ONLY, REGISTRY_AND_DATA, DATA_NO_REGISTRY, MIXED)
# (input class, change set), run before the drawn ones.
EXAMPLES = [
    (REGISTRY_AND_DATA, ROWS_ONLY_DIFF),
    (MIXED, ROWS_ONLY_DIFF + ["crates/holon-integration-tests/src/pbt/known_reds.rs"]),
    (MIXED, [REGISTRY, "scripts/keystone-known-reds.sh"]),
    (DATA_NO_REGISTRY, ROWS_ONLY_DIFF[1:]),
    (REGISTRY_ONLY, [REGISTRY]),
    (REGISTRY_AND_DATA, [REGISTRY, GONE_EVIDENCE]),
    (DATA_NO_REGISTRY, [GONE_EVIDENCE]),
    (DATA_NO_REGISTRY, []),
    *[(MIXED, [REGISTRY, p]) for p in NEAR_MISS],
]


def is_data(path):
    return path in (REGISTRY, FEATUREMAP) or path.startswith(EVIDENCE_DIR)


def classify(changed):
    if not all(is_data(p) for p in changed):
        return MIXED
    if REGISTRY not in changed:
        return DATA_NO_REGISTRY
    return REGISTRY_ONLY if changed == [REGISTRY] else REGISTRY_AND_DATA


def pools(meta):
    """{pool: sorted paths}: tracked files from the evidence check's listing of every file this tree's commit carries,
    plus paths a diff can hold that the tree does not (a deleted evidence file, near misses)."""
    dirs, _ = st.workspace_graph(meta)
    files = sorted(p for p in evidence.landing_paths() if (REPO / p).is_file())
    assert REGISTRY in files, f"{REGISTRY} is not tracked in {REPO}"
    crate_src, crate_tests = [], []
    for p in files:
        o = st.owner(PurePosixPath(p), dirs)
        if o and p.endswith(".rs"):
            (crate_tests if dirs[o] / "tests" in PurePosixPath(p).parents else crate_src).append(p)
    found = {
        "data": [p for p in files if is_data(p) and p != REGISTRY] + [GONE_EVIDENCE],
        "docs": [p for p in files if p.startswith("docs/") and not is_data(p)],
        "near miss": NEAR_MISS,
        "scripts": [p for p in files if p.startswith("scripts/")],
        "crate src": crate_src,
        "crate tests": crate_tests,
        "root": [p for p in files if "/" not in p or p.startswith(".cargo/")],
    }
    empty = [k for k, v in found.items() if not v]
    assert not empty, f"pools {empty} are empty"
    return found


def draw(rng, pool):
    return rng.sample(pool, rng.randint(0, min(3, len(pool))))


def draw_case(rng, pools_, cls):
    """(input class, sub-shape, change set) of input class `cls`; a mixed set holds a non-data path."""
    shape = cls
    if cls == REGISTRY_ONLY:
        changed = [REGISTRY]
    elif cls == REGISTRY_AND_DATA:
        changed = [REGISTRY] + rng.sample(pools_["data"], rng.randint(1, 3))
    elif cls == DATA_NO_REGISTRY:
        changed = draw(rng, pools_["data"])
    elif rng.random() < 0.5:
        shape = "registry + one inert non-data path"
        changed = [REGISTRY, rng.choice(pools_[rng.choice(["docs", "scripts", "near miss"])])]
    else:
        changed = ([REGISTRY] if rng.random() < 0.5 else []) + draw(rng, pools_["data"])
        for k in (k for k in pools_ if k != "data"):
            # A root build file selects every crate; drawn as often as the others it would mask the narrower lines.
            changed += draw(rng, pools_[k]) if k != "root" or rng.random() < 0.15 else []
        if all(is_data(p) for p in changed):
            changed.append(rng.choice(pools_[rng.choice(["docs", "near miss", "scripts", "crate src"])]))
    rng.shuffle(changed)
    return cls, shape, changed


def run(sut, changed, **rule):
    err = io.StringIO()
    with contextlib.redirect_stderr(err):
        try:
            lines = sut.selection(REPO, META, list(changed), **rule)
        except ValueError as e:
            lines = f"ValueError: {e}"
    return lines, err.getvalue()


def expected_registry_lines():
    canon = "--workspace --features " + ",".join(st.canon_features(REPO))
    return [f"clippy: {canon}",
            f"nextest: {canon} --no-tests=fail --profile load-tolerant"
            " -E 'package(holon-integration-tests) & kind(lib) & test(/^pbt::known_reds::/)'",
            "known-reds-fixture: just known-reds-fixture"]


def full_selection_problems(changed, lines):
    found = []
    if "registry_data" not in inspect.signature(st.selection).parameters:
        found.append("P2: selection() has no `registry_data` parameter to switch the rule off")
    elif lines != (off := run(st, changed, registry_data=frozenset())[0]):
        found.append(f"P2: prints {lines}, with the rule off {off}")
    if BASE:
        old = run(BASE, changed)[0]
        if lines != old:
            found.append(f"P2: prints {lines}, the base selector {old}")
    return found


def problems(rng, cls, changed, pools_):
    assert classify(changed) == cls, (cls, changed)
    found = []
    lines, err = run(st, changed)
    if lines != run(st, list(reversed(changed)))[0]:
        found.append("the lines change with the order of the changed paths")
    if cls in (REGISTRY_ONLY, REGISTRY_AND_DATA):
        want = expected_registry_lines()
        if lines != want:
            extra = [m for m in ("package(holon-gpui)", "package(holon-tui)", "hand-authored:", "windowed:", "timing:")
                     if any(m in line for line in lines)]
            found.append(f"P1: prints {lines} (holds {extra}), expected {want}")
        rule_line = f"registry data only: {len(changed)} paths; the registry's own checks"
        if rule_line not in err:
            found.append(f"P1: stderr does not name the rule ({rule_line!r}): {err.strip()!r}")
        return found
    found += full_selection_problems(changed, lines)
    if cls == DATA_NO_REGISTRY:
        grown = changed + [p for p in [rng.choice(pools_["data"])] if p not in changed]
        found += [f"P3, grown to {grown}: {f}" for f in full_selection_problems(grown, run(st, grown)[0])]
    return found


def integrate_problems():
    """The integrate gate keeps its full run for registry data: the tour and the hand-authored cases."""
    found = []
    for changed in ([REGISTRY], ROWS_ONLY_DIFF):
        with contextlib.redirect_stderr(io.StringIO()):
            got = {step for step, _, _ in st.quiet_gate_selection(REPO, META, "integrate", changed)}
        for step in ("keystone-tour", "hand-authored"):
            if step not in got:
                found.append(f"integrate gate, {changed}: no {step} step in {sorted(got)}")
    return found


def base_selector(rev):
    if not rev:
        return None
    text = subprocess.run(["jj", "--ignore-working-copy", "file", "show", "-r", rev, "scripts/select-tests.py"],
                          cwd=REPO, check=True, stdout=subprocess.PIPE, text=True).stdout
    path = Path(tempfile.mkdtemp()) / "select_tests_base.py"
    path.write_text(text)
    return load("select_tests_base", path)


META = st.load_metadata()
BASE = base_selector(os.environ.get("SELECT_TESTS_BASE"))


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**32)))
    rng = random.Random(seed)
    pools_ = pools(META)
    # Classes cycle so each holds a quarter of the draws; only the paths are random.
    drawn = [draw_case(rng, pools_, CLASSES[i % len(CLASSES)]) for i in range(int(os.environ.get("CASES", "40")))]
    print(f"drawn with seed {seed}; pool sizes {({k: len(v) for k, v in pools_.items()})}")
    print(f"base selector: {os.environ.get('SELECT_TESTS_BASE') or 'none (SELECT_TESTS_BASE unset)'}")
    failures = integrate_problems()
    for cls, changed in EXAMPLES + [(cls, changed) for cls, _, changed in drawn]:
        for f in problems(rng, cls, changed, pools_):
            failures.append(f"({cls!r}, {changed}): {f}")
    classes = Counter(cls for cls, _, _ in drawn)
    print(f"drawn cases per input class: {dict(classes)}; sub-shapes: {dict(Counter(s for _, s, _ in drawn))}")
    for f in failures:
        print(f"FAIL: {f}")
    print(f"test-select-tests-registry-data: {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
