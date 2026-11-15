#!/usr/bin/env python3
"""Property check of the crates scripts/select-tests.py selects through dev-dependencies.

Run from the workspace root: `python3 scripts/test-select-tests-dev-edges.py` (SEED=<n>, CASES=<n>, default 400).
The oracle is a forward walk (`dependents_oracle`): a crate's tests link its dependencies of every kind and, from
there on, only normal and build dependencies. For each change set, the selected crates (the integration crate counted
when its binaries are named) are exactly:
- every owner of a changed path whose changed paths all sit under its `tests/`, plus
- every crate whose tests link a crate owning another changed path (sound: no linking test is dropped),
and never more than the walk over every edge kind selects. A changed `Cargo.toml` selects that wider walk.
Fixed cases first: two landed diffs on this workspace's own `cargo metadata`. Then drawn workspace graphs, whose
normal and build edges point to earlier crates and whose dev edges point anywhere (cargo allows dev cycles).
Exits non-zero when a property fails or a named input class was never drawn; a failing case prints its graph.
"""
import contextlib
import importlib.util
import io
import os
import random
import sys
from collections import Counter
from pathlib import Path, PurePosixPath

REPO = Path(__file__).resolve().parent.parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


st = load("select_tests", REPO / "scripts/select-tests.py")
# Changed paths of two landed diffs: a change in `holon`, and one in `holon-mcp-client`.
EXAMPLES = [
    ["crates/holon/src/core/sql_operation_provider.rs", "crates/holon/tests/main.rs",
     "crates/holon/tests/sql_provider_atomic_writes.rs"],
    ["crates/holon-mcp-client/src/secure_client.rs", "crates/holon-mcp-client/tests/http_bounds.rs",
     "crates/holon-mcp-client/tests/redirect_downgrade.rs",
     "docs/Testing/bugfunnel/entries/2026-11-14-a-redirect-moves-a-connector-key-header-to-another-host.md"],
]


def selected(meta, changed):
    with contextlib.redirect_stderr(io.StringIO()):
        pkgs, _, bins = st.compute(meta, changed, test_binaries=lambda: {"integration": []})
    return set(pkgs) | ({st.INTEGRATION} if bins else set())


def expected(meta, changed):
    """(the crates the oracle selects, the crates the walk over every edge kind selects)."""
    dirs, _ = st.workspace_graph(meta)
    paths = [PurePosixPath(c) for c in changed]
    owned = {}
    for p in paths:
        if (o := st.owner(p, dirs)) is not None:
            owned.setdefault(o, []).append(p)
    outside = {o for o, ps in owned.items() if any(dirs[o] / "tests" not in p.parents for p in ps)}
    manifest = any(p.name == "Cargo.toml" for p in paths)
    link, every = st.dependents_oracle(meta, through_dev=manifest), st.dependents_oracle(meta, through_dev=True)
    want = set(owned) | {x for o in outside for x in link[o]}
    wide = set(owned) | {x for o in outside for x in every[o]}
    return want, wide


def draw_meta(rng):
    names = [f"c{i}" for i in range(rng.randint(3, 8))]
    names[rng.randrange(len(names))] = st.INTEGRATION
    deps = {n: [] for n in names}
    for i, n in enumerate(names):
        for j, d in enumerate(names):
            if i == j or rng.random() > 0.35:
                continue
            kind = "dev" if j > i else rng.choice([None, None, "build", "dev"])
            deps[n].append({"name": d, "kind": kind})
            if kind == "dev" and j < i and rng.random() < 0.2:
                deps[n].insert(rng.choice([len(deps[n]) - 1, len(deps[n])]), {"name": d, "kind": None})
    return {"workspace_root": "/w", "workspace_members": names,
            "packages": [{"name": n, "id": n, "manifest_path": f"/w/crates/{n}/Cargo.toml", "dependencies": deps[n]}
                         for n in names]}


def draw_changed(rng, meta):
    crates = [p["name"] for p in meta["packages"]]
    return sorted({f"crates/{c}/" + rng.choice(["src/lib.rs", "src/a/b.rs", "tests/t.rs", "Cargo.toml"])
                   if rng.random() < 0.15 else f"crates/{c}/" + rng.choice(["src/lib.rs", "tests/t.rs"])
                   for c in rng.sample(crates, rng.randint(1, min(3, len(crates))))} | (
                      {"docs/x.md"} if rng.random() < 0.2 else set()))


def problems(meta, changed, counts):
    got = selected(meta, changed)
    want, wide = expected(meta, changed)
    counts["cases"] += 1
    counts["dev edge narrows"] += bool(wide - want)
    counts["changed manifest"] += any(c.endswith("Cargo.toml") for c in changed)
    counts["integration selected"] += st.INTEGRATION in got
    counts["normal edge listed before dev edge"] += any(
        a["name"] == b["name"] and a["kind"] is None and b["kind"] == "dev"
        for p in meta["packages"] for k, a in enumerate(p["dependencies"]) for b in p["dependencies"][k + 1:])
    found = []
    if got != want:
        found.append(f"selects {sorted(got)}; the tests linking a changed crate are {sorted(want)}: "
                     f"missing {sorted(want - got)}, extra {sorted(got - want)}")
    if not got <= wide:
        found.append(f"selects {sorted(got - wide)} beyond the walk over every edge kind")
    return found


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**32)))
    rng = random.Random(seed)
    counts, failures = Counter(), 0
    meta = st.load_metadata()
    for changed in EXAMPLES:
        found = problems(meta, changed, Counter())
        want, wide = expected(meta, changed)
        print(f"example {changed[0]}: selects {len(want)} crates; the walk over every edge kind selects {len(wide)};"
              f" dropped {sorted(wide - want)}")
        for f in found:
            print(f"FAIL: EXAMPLES entry {changed}: {f}")
        failures += len(found)
    for _ in range(int(os.environ.get("CASES", "400"))):
        drawn = draw_meta(rng)
        changed = draw_changed(rng, drawn)
        for f in problems(drawn, changed, counts):
            graph = {p["name"]: [(d["name"], d["kind"]) for d in p["dependencies"]] for p in drawn["packages"]}
            print(f"FAIL: graph {graph}, changed {changed}: {f}")
            failures += 1
    print(f"drawn with seed {seed}: {dict(counts)}")
    for cls in ("dev edge narrows", "changed manifest", "integration selected", "normal edge listed before dev edge"):
        if not counts[cls]:
            print(f"FAIL: no case of class `{cls}` was drawn")
            failures += 1
    print(f"test-select-tests-dev-edges: {failures} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
