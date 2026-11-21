#!/usr/bin/env python3
"""Self-check for the recipe-inventory check of scripts/timing-split.py `resolve` and its gate-owned machinery check.

Run: `python3 scripts/test-timing-split.py`. Exits non-zero on the first violation.
The machinery check is a full matrix, not a generator: its inputs are four small finite axes (row owner, module
exemption, the one machinery path left uncovered, its kind), and every combination is drawn.
"""
import contextlib
import datetime
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


# Fixed lines, one per way a row can be malformed: the reader's input space is these shapes
# (wrong column count, non-numeric seconds/load), not their combinations.
MALFORMED_ROWS = (
    "garbage",
    "x\ty",
    "2026-11-01\tc0\ttest\tNOTNUM\t1.0\t2.0\tpass",
    "2026-11-01\tc0\ttest\t5.0\t1.0\t2.0\tpass\textra",
)


def run_p95_with(t, ledger_writer):
    saved = t.LEDGER, t.datetime
    err = io.StringIO()
    try:
        with tempfile.TemporaryDirectory() as tmp:
            t.LEDGER = Path(tmp) / "test-duration-ledger.tsv"
            ledger_writer(t.LEDGER)
            t.datetime = FixedDate(datetime.date(2026, 11, 20))
            with contextlib.redirect_stderr(err):
                code, out = run(t.cmd_p95, None)
    finally:
        t.LEDGER, t.datetime = saved
    return code, out, err.getvalue()


def p95_discloses_dropped_rows_and_bad_ledgers(t):
    for n in range(1, len(MALFORMED_ROWS) + 1):
        rows = MALFORMED_ROWS[:n]
        code, out, err = run_p95_with(t, lambda p: p.write_text("\n".join(rows) + "\n"))
        assert code == 0 and f"dropped {n} malformed" in err, (n, code, err)
    code, out, err = run_p95_with(t, lambda p: p.mkdir())
    assert code == 0 and "not a file" in err, (code, err)
    code, out, err = run_p95_with(t, lambda p: p.write_bytes(b"\x8e\xff\xfe"))
    assert code == 0 and "unreadable" in err, (code, err)
    code, out, err = run_p95_with(t, lambda p: None)
    assert code == 0 and err == "", ("a missing ledger is the normal fresh state", err)


def p95_names_a_dangling_symlink_ledger(t):
    def plant(p):
        p.symlink_to(p.parent / "gone")
    code, _out, err = run_p95_with(t, plant)
    assert code == 0 and "not a file" in err, (code, err)


def seed_refuses_a_non_finite_duration(t):
    for value in ("nan", "inf", "-inf", "1e999"):
        for column in ("secs", "load_start", "load_end"):
            with tempfile.TemporaryDirectory() as tmp:
                tmp = Path(tmp)
                log = tmp / "run.log"
                log.write_text("")
                row = {"pass": "1", "test": "a::b", "leg": "x", "exit": "0", "result": "green",
                       "load_start": "1.0", "load_end": "2.0", "secs": "3.0", "log": str(log)}
                row[column] = value
                csv_path = tmp / "results.csv"
                csv_path.write_text(",".join(row) + "\n" + ",".join(row.values()) + "\n")
                saved = t.LEDGER
                t.LEDGER = tmp / "ledger.tsv"
                try:
                    with contextlib.redirect_stderr(io.StringIO()):
                        try:
                            t.cmd_seed(str(csv_path))
                        except SystemExit as e:
                            assert e.code != 0, (value, column)
                        else:
                            raise AssertionError(f"seed accepted {column}={value}")
                    assert not t.LEDGER.exists() or "inf" not in t.LEDGER.read_text() and "nan" not in t.LEDGER.read_text()
                finally:
                    t.LEDGER = saved


class FixedDate:
    def __init__(self, today):
        self.date = type("D", (datetime.date,), {"today": staticmethod(lambda: today)})
        self.timedelta = datetime.timedelta


def planted_ledger(t, tmp, secs):
    """Rows that put every capped test's p95_low at `secs`; dated before every `today` the test uses."""
    path = Path(tmp) / "test-duration-ledger.tsv"
    runs = t.MIN_RUNS_FOR_P95
    path.write_text("".join(
        f"2026-11-01\tc{i}\t{test}\t{secs}\t1.0\t2.0\tpass\n"
        for test, *_ in t.cap_rows() for i in range(runs)))
    return path


def run(fn, *args):
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = fn(*args)
    return code, out.getvalue()


# Fixed cases: the input is one small file shape (a planted ledger), and the axis that matters is the
# calendar day, which three values cover (inside the window twice, and past it).
def check_ignores_the_ledger_and_the_date(t):
    saved = t.LEDGER, t.datetime
    verdicts = set()
    try:
        with tempfile.TemporaryDirectory() as tmp:
            t.LEDGER = planted_ledger(t, tmp, 40)
            for test, _p95, cap, _ in t.cap_rows():
                assert not t.in_clamp(cap, 40), f"the planted p95_low must leave {test}'s clamp, or the test proves nothing"
            for today in (datetime.date(2026, 11, 2), datetime.date(2026, 11, 20), datetime.date(2027, 3, 1)):
                t.datetime = FixedDate(today)
                code, out = run(t.cmd_check)
                assert "timing-split: PASS" in out and code == 0, f"check on {today} must pass on the tree alone: {out}"
                verdicts.add((code, out))
    finally:
        t.LEDGER, t.datetime = saved
    assert len(verdicts) == 1, "check output must not depend on the date"


def p95_reports_the_re_derivation_without_failing(t):
    saved = t.LEDGER, t.datetime
    try:
        with tempfile.TemporaryDirectory() as tmp:
            t.LEDGER = planted_ledger(t, tmp, 40)
            t.datetime = FixedDate(datetime.date(2026, 11, 20))
            code, out = run(t.cmd_p95, None)
    finally:
        t.LEDGER, t.datetime = saved
    assert code == 0, out
    for test, _p95, cap, _ in t.cap_rows():
        assert f"{test}: p95_low 40.0s -> cap {t.cap_for(40)}s" in out, out
        assert f"cap {cap:.0f}s has left the clamp" in out, out


def unreadable_ledger_variants():
    """Class generator: ledger files that exist but that no row parser can read as text."""
    yield b"\x8e\xff\xfe not utf-8 \x80"
    yield b"\xc3\x28\n"
    yield bytes(range(128, 256))


def p95_never_fails_on_an_unreadable_ledger(t):
    saved = t.LEDGER, t.datetime
    try:
        for blob in unreadable_ledger_variants():
            with tempfile.TemporaryDirectory() as tmp:
                t.LEDGER = Path(tmp) / "test-duration-ledger.tsv"
                t.LEDGER.write_bytes(blob)
                code, _out = run(t.cmd_p95, None)
                assert code == 0, blob
    finally:
        t.LEDGER, t.datetime = saved


def non_finite_second_values():
    """Class generator: seconds fields that float() accepts but that are no duration."""
    yield from ("nan", "NaN", "inf", "-inf", "1e999", "1e308")


def p95_never_fails_on_a_non_finite_duration(t):
    capped = t.cap_rows()[0][0]
    for value in non_finite_second_values():
        def plant(p, value=value):
            row = f"2026-11-20\tc0\t{capped}\t{value}\t0.5\t0.6\tpass\n"
            p.write_text(row * 12)
        code, _out, err = run_p95_with(t, plant)
        assert code == 0, (value, code, err)


def arbitrary_seconds_values(n=200):
    """Class generator: floats of every magnitude and sign, plus huge, tiny, negative, subnormal."""
    import random, struct
    rng = random.Random(546)
    yield from ("-5", "-0.001", "5e-324", "1e300", "1e9", "1e308", "nan", "inf", "-inf")
    for _ in range(n):
        bits = rng.getrandbits(64)
        yield repr(struct.unpack("<d", struct.pack("<Q", bits))[0])


def p95_counts_out_of_range_seconds_and_keeps_in_range_ones(t):
    capped = t.cap_rows()[0][0]
    for value in arbitrary_seconds_values():
        v = float(value)
        in_range = v == v and 0 <= v <= 7 * 24 * 3600
        def plant(p, value=value):
            p.write_text(f"2026-11-20\tc0\t{capped}\t{value}\t0.5\t0.6\tpass\n" * 12)
        code, out, err = run_p95_with(t, plant)
        assert code == 0, (value, code, err)
        assert ("dropped" in err) == (not in_range), (value, in_range, err)


def main():
    t = load_module()
    check_ignores_the_ledger_and_the_date(t)
    p95_reports_the_re_derivation_without_failing(t)
    p95_never_fails_on_an_unreadable_ledger(t)
    p95_counts_out_of_range_seconds_and_keeps_in_range_ones(t)
    st = t.select_tests()
    machinery_matrix_matches_the_reference(t, st)
    missing_module_file_is_named(t, st)
    keystone_machinery_outside_pbt_is_red_on_the_old_table(t, st)
    keystone_owned_by_a_lane_run_recipe_is_green(t, st)
    p95_names_a_dangling_symlink_ledger(t)
    seed_refuses_a_non_finite_duration(t)
    inventory_inside_its_group_stays_green(t)
    group_without_inventory_is_not_judged(t)
    inventory_test_outside_its_group_is_named(t)
    inventory_of_an_unknown_group_dies(t)
    inventory_is_read_from_the_windowed_gate(t)
    p95_discloses_dropped_rows_and_bad_ledgers(t)
    p95_never_fails_on_a_non_finite_duration(t)
    print("test-timing-split: OK")


if __name__ == "__main__":
    main()
