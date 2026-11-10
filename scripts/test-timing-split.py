#!/usr/bin/env python3
"""Self-check for the recipe-inventory check of scripts/timing-split.py `resolve` and its gate-owned machinery check.

Run: `python3 scripts/test-timing-split.py`. Exits non-zero on the first violation.
The machinery check is a full matrix, not a generator: its inputs are four small finite axes (row owner, module
exemption, the one machinery path left uncovered, its kind), and every combination is drawn.
"""
import contextlib
import importlib.util
import io
import itertools
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "timing-split.py"
GROUPS = {"windowed": 4, "cost-sensitive": 1}
INVENTORIES = {"windowed": ("just windowed-gate", "package(holon-gpui)")}


def load_module():
    spec = importlib.util.spec_from_file_location("timing_split", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def asked(t, outside_of):
    calls = []

    def outside(filt, group):
        calls.append((filt, group))
        return outside_of.get(group, [])

    return t.inventory_gaps(GROUPS, INVENTORIES, outside), calls


def inventory_inside_its_group_stays_green(t):
    gaps, _ = asked(t, {})
    assert gaps == {"windowed": []}, f"an inventory inside its group must report no test outside it: {gaps}"


def group_without_inventory_is_not_judged(t):
    gaps, calls = asked(t, {"cost-sensitive": ["never asked"]})
    assert "cost-sensitive" not in gaps, f"a group with no recipe inventory must not be judged: {gaps}"
    assert calls == [("package(holon-gpui)", "windowed")], f"only the windowed inventory may be listed: {calls}"


def inventory_test_outside_its_group_is_named(t):
    gaps, _ = asked(t, {"windowed": ["holon-gpui::main gpui_rebind_reset_smoke::a"]})
    assert gaps == {"windowed": ["holon-gpui::main gpui_rebind_reset_smoke::a"]}, gaps


def inventory_of_an_unknown_group_dies(t):
    err = io.StringIO()
    try:
        with contextlib.redirect_stderr(err):
            t.inventory_gaps({"cost-sensitive": 1}, INVENTORIES, lambda f, g: [])
    except SystemExit:
        assert "['windowed']" in err.getvalue(), err.getvalue()
        return
    raise AssertionError("an inventory whose group the groups table lacks must die, not skip the check")


def inventory_is_read_from_the_windowed_gate(t):
    got = t.recipe_inventories()
    assert got == INVENTORIES, f"recipe_inventories() must read windowed-gate.py's INVENTORY: {got}"


PKG = "itc"
CRATE = f"crates/{PKG}"
LANE_RUN, GATE_ONLY = "just keystone-smoke", "just timing-gate"
EXEMPT, CHECKED = "latency_slo_gate", "keystone"
SRC, PBT = f"{CRATE}/src/lib.rs", f"{CRATE}/src/pbt/step.rs"
OTHER = "crates/other/src/lib.rs"


def entry(module):
    return f"{CRATE}/tests/{module}.rs"


def term(module):
    return f"binary_id({PKG}::integration) & test(/^{module}::/)"


def machinery_tree(root):
    for path in (SRC, PBT, OTHER, entry(EXEMPT), entry(CHECKED)):
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_text("")


# (owner, module, uncovered kind) -> the paths the check names.
EXPECTED = {
    (owner, module, kind): ([entry(module) if kind == "entry" else {"src": SRC, "pbt": PBT}[kind]]
                            if owner == GATE_ONLY and module == CHECKED and kind != "other" else [])
    for owner, module, kind in itertools.product((LANE_RUN, GATE_ONLY), (EXEMPT, CHECKED),
                                                 ("entry", "src", "pbt", "other"))
}


def machinery_gaps(t, st, root, owners, path_owners):
    _rows, _files, problems = t.gate_owned_machinery_gaps(root, owners, path_owners, st)
    return problems


def machinery_matrix_matches_the_reference(t, st):
    assert EXEMPT in st.NEVER_OWNER_MODULES and CHECKED not in st.NEVER_OWNER_MODULES, st.NEVER_OWNER_MODULES
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        machinery_tree(root)
        for (owner, module, kind), want in EXPECTED.items():
            uncovered = {"entry": entry(module), "src": SRC, "pbt": PBT, "other": OTHER}[kind]
            covered = [p for p in (entry(module), SRC, PBT, OTHER) if p != uncovered]
            path_owners = {p: "keystone-smoke" for p in covered} | {uncovered: "timing"}
            got = machinery_gaps(t, st, root, {term(module): owner}, path_owners)
            named = [p for p in (entry(module), SRC, PBT, OTHER) if any(f" {p} selects" in g for g in got)]
            assert named == want and len(got) == len(want), f"{owner} {module} uncovered {kind}: {got}"


def missing_module_file_is_named(t, st):
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        machinery_tree(root)
        got = machinery_gaps(t, st, root, {term("gone"): GATE_ONLY}, {f"{CRATE}/": "keystone-smoke"})
    assert got and f"{CRATE}/tests/gone.rs does not exist" in got[0], got


KEYSTONE = "binary_id(holon-integration-tests::integration) & test(/^general_e2e_composed_pbt::/)"
OLD_PATHS = {
    "crates/holon-integration-tests/src/pbt/": "keystone-smoke",
    "crates/holon-integration-tests/tests/hand_authored_regressions.rs": "keystone-smoke",
}


def keystone_machinery_outside_pbt_is_red_on_the_old_table(t, st):
    crate = t.REPO / "crates/holon-integration-tests"
    want = sorted(["crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs"]
                  + [p.relative_to(t.REPO).as_posix() for p in (crate / "src").rglob("*")
                     if p.is_file() and "pbt" not in p.relative_to(crate / "src").parts[:1]])
    got = machinery_gaps(t, st, t.REPO, {KEYSTONE: GATE_ONLY}, OLD_PATHS)
    named = sorted(g.split(", and ")[1].removesuffix(" selects no lane-run recipe") for g in got)
    assert named == want, f"want {len(want)} paths, got {len(named)}: {sorted(set(named) ^ set(want))}"


def keystone_owned_by_a_lane_run_recipe_is_green(t, st):
    got = machinery_gaps(t, st, t.REPO, {KEYSTONE: LANE_RUN}, OLD_PATHS)
    assert got == [], got


def main():
    t = load_module()
    st = t.select_tests()
    machinery_matrix_matches_the_reference(t, st)
    missing_module_file_is_named(t, st)
    keystone_machinery_outside_pbt_is_red_on_the_old_table(t, st)
    keystone_owned_by_a_lane_run_recipe_is_green(t, st)
    inventory_inside_its_group_stays_green(t)
    group_without_inventory_is_not_judged(t)
    inventory_test_outside_its_group_is_named(t)
    inventory_of_an_unknown_group_dies(t)
    inventory_is_read_from_the_windowed_gate(t)
    print("test-timing-split: OK")


if __name__ == "__main__":
    main()
