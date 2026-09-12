#!/usr/bin/env python3
"""Self-check for the clock-origin split in the latency log parsers (D119.a).

The Rust side keeps the two origins apart with types — a `SloWindow` holds one
origin and refuses a foreign sample. The Python side has no type system to lean
on, and that is exactly where the split leaked: `tight_arm.py` kept filtering on
`action` alone after `read_e2e` gained an `origin` key, and pooled UI and facade
samples into one p95.

So the invariants these scripts must hold are asserted here instead:

  1. `read_e2e` cannot hand out an unsplit population — the origin is a REQUIRED
     positional argument, so a consumer that forgets it gets a TypeError rather
     than a pooled distribution.
  2. It returns only the named origin's samples.
  3. A pre-D119.a log (no `origin` field) FAILS LOUD in every parser, naming the
     log. Reporting zero samples from such a log is the silent-degradation case.
  4. `measure_latency.py` reports the two origins as separate tables.

Run: `python3 scripts/latency/selfcheck.py` (or `just latency-scripts-selfcheck`).
Exits non-zero on the first violation.
"""
import pathlib
import re
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(HERE))

from analyze_spans import load as analyze_load  # noqa: E402
from measure_arms import read_e2e  # noqa: E402


CORRELATOR = REPO / "crates" / "holon-api" / "src" / "latency_e2e.rs"

# How each field of the `stage="e2e"` event renders in a log line. `tracing`
# quotes a `&str` value and leaves a `%`-Display, numeric or bool value bare, so
# this table says which is which. The ORDER is never written here — it is read
# from the emitter (see `emitter_field_order`), because a reordering is exactly
# the change a hand-written fixture would hide.
FIELD_RENDER = {
    "stage": ("quoted", "e2e"),
    "action": ("bare", None),      # `%c.action`
    "block": ("bare", None),       # `%c.target`
    "origin": ("quoted", None),
    "source": ("quoted", "block"),
    "ms": ("bare", None),
    "in_flight": ("bare", 1),
    "backlog": ("bare", 0),
    "contended": ("bare", "false"),
}


def emitter_field_order():
    """The `stage="e2e"` event's field names, in the order the emitter writes.

    Derived from the correlator source rather than restated here. A fixture
    written by hand agrees with itself forever: reorder the emission so that
    `source` lands after `ms` and every regex in these scripts would break
    against a real log while the self-check stayed green — which would make this
    file a decoration. Reading the emitter is what gives it teeth.
    """
    src = CORRELATOR.read_text()
    start = src.index('stage = "e2e",')
    end = src.index('"holon_latency",', start)
    names = re.findall(r"^\s{12}(\w+) =", src[start:end], re.M)
    names = ["stage"] + [n for n in names if n != "stage"]
    unknown = [n for n in names if n not in FIELD_RENDER]
    if unknown:
        raise SystemExit(
            f"{CORRELATOR}: the e2e emission has field(s) this self-check does not know how to "
            f"render: {unknown}. Add them to FIELD_RENDER (and check whether the log parsers "
            f"need updating too) — a new field must not slip past the fixture."
        )
    missing = [n for n in FIELD_RENDER if n not in names]
    if missing:
        raise SystemExit(
            f"{CORRELATOR}: the e2e emission no longer carries {missing}, but this self-check "
            f"still expects them. The parsers in scripts/latency/ almost certainly need updating."
        )
    return names


FIELD_ORDER = emitter_field_order()


def line(action, block, origin, ms, ts="2026-09-12T19:07:57.247228Z"):
    """One `stage="e2e"` log line, built in the emitter's own field order."""
    values = {"action": action, "block": block, "origin": origin, "ms": ms}
    parts = []
    for name in FIELD_ORDER:
        kind, default = FIELD_RENDER[name]
        v = values.get(name, default)
        parts.append(f'{name}="{v}"' if kind == "quoted" else f"{name}={v}")
    return f"{ts}  INFO holon_latency: holon_latency " + " ".join(parts) + "\n"


def legacy_line(action, block, ms, ts="2026-08-08T05:50:50.871572Z"):
    """The pre-D119.a emission: no `origin` between `block=` and `source=`."""
    return (
        f'{ts}  INFO holon_latency: holon_latency stage="e2e" action={action} '
        f'block={block} source="block" ms={ms}\n'
    )


def write(tmp, name, text):
    p = pathlib.Path(tmp) / name
    p.write_text(text)
    return str(p)


FAILURES = []


def check(name, fn):
    try:
        fn()
    except AssertionError as e:
        FAILURES.append(f"{name}: {e}")
    except Exception as e:  # a parser that blew up is also a failure
        FAILURES.append(f"{name}: unexpected {type(e).__name__}: {e}")
    else:
        print(f"  ok  {name}")


def main():
    with tempfile.TemporaryDirectory() as tmp:
        mixed = write(
            tmp,
            "mixed.log",
            line("set_field", "block:a", "ui", 100)
            + line("set_field", "block:b", "facade", 5)
            + line("set_field", "block:c", "ui", 110)
            + line("set_field", "block:d", "facade", 7),
        )
        # `analyze_spans` only ever looks at `set_field`, so the pre-D119.a log
        # has to carry one for that parser's check to be about the missing
        # origin rather than about the action.
        old = write(
            tmp,
            "old.log",
            legacy_line("set_field", "block:bench-18", 32)
            + legacy_line("split_block", "block:bench-17", 34),
        )

        def origin_is_required():
            try:
                read_e2e(mixed)  # noqa: E1120 — the point of the check
            except TypeError:
                return
            raise AssertionError(
                "read_e2e accepted a call with no origin — a consumer can still "
                "obtain a pooled population and take one percentile over it"
            )

        def only_the_named_origin():
            ui = read_e2e(mixed, "ui")
            facade = read_e2e(mixed, "facade")
            assert [e["ms"] for e in ui] == [100, 110], f"ui population wrong: {ui}"
            assert [e["ms"] for e in facade] == [5, 7], f"facade population wrong: {facade}"

        def unknown_origin_refused():
            try:
                read_e2e(mixed, "ui-ish")
            except ValueError:
                return
            raise AssertionError("read_e2e accepted an origin that names no clock seam")

        def measure_arms_fails_loud_on_an_old_log():
            try:
                read_e2e(old, "ui")
            except SystemExit as e:
                assert old in str(e), f"the error must name the log, got: {e}"
                return
            raise AssertionError(
                "a pre-D119.a log yielded a silent empty population instead of an error"
            )

        def analyze_spans_fails_loud_on_an_old_log():
            try:
                analyze_load(old)
            except SystemExit as e:
                assert old in str(e), f"the error must name the log, got: {e}"
                return
            raise AssertionError(
                "a pre-D119.a log yielded a silent empty decomposition instead of an error"
            )

        def measure_latency_splits_the_tables():
            out = subprocess.run(
                [sys.executable, str(REPO / "scripts" / "measure_latency.py"), mixed],
                capture_output=True,
                text=True,
                check=True,
            ).stdout
            assert "origin=ui" in out, "no UI table in the report"
            assert "origin=facade" in out, "no facade table in the report"
            ui_at = out.index("origin=ui")
            facade_at = out.index("origin=facade")
            assert ui_at < facade_at, "the UI table must come first"

        def measure_latency_discloses_an_originless_event():
            out = subprocess.run(
                [sys.executable, str(REPO / "scripts" / "measure_latency.py"), old],
                capture_output=True,
                text=True,
                check=True,
            ).stdout
            assert "SCORED NOWHERE" in out, (
                "an `e2e` event with no origin must be disclosed, not dropped in "
                f"silence. Output was:\n{out}"
            )

        print("latency script self-check (D119.a clock-origin split)")
        check("read_e2e requires an origin", origin_is_required)
        check("read_e2e returns only that origin", only_the_named_origin)
        check("read_e2e refuses an unknown origin", unknown_origin_refused)
        check("measure_arms fails loud on a pre-D119.a log", measure_arms_fails_loud_on_an_old_log)
        check("analyze_spans fails loud on a pre-D119.a log", analyze_spans_fails_loud_on_an_old_log)
        check("measure_latency reports two tables", measure_latency_splits_the_tables)
        check("measure_latency discloses an originless event", measure_latency_discloses_an_originless_event)

    if FAILURES:
        print("\nFAILED:", file=sys.stderr)
        for f in FAILURES:
            print(f"  - {f}", file=sys.stderr)
        sys.exit(1)
    print("all checks passed")


if __name__ == "__main__":
    main()
