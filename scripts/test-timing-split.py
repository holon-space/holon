#!/usr/bin/env python3
"""Self-check for the recipe-inventory check of scripts/timing-split.py `resolve`.

Run: `python3 scripts/test-timing-split.py`. Exits non-zero on the first violation.
"""
import contextlib
import importlib.util
import io
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


def main():
    t = load_module()
    inventory_inside_its_group_stays_green(t)
    group_without_inventory_is_not_judged(t)
    inventory_test_outside_its_group_is_named(t)
    inventory_of_an_unknown_group_dies(t)
    inventory_is_read_from_the_windowed_gate(t)
    print("test-timing-split: OK")


if __name__ == "__main__":
    main()
