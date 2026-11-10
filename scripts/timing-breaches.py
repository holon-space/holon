#!/usr/bin/env python3
"""The breach summary of the timing ledger, one block per gate.

Every `small` DurationBudget check appends one line to the ledger
(crates/holon-api/src/timing_mode.rs). This script reads it and prints, per
gate, the over-budget checks and a `TREND` line for each (test, quantity) that
is over budget in its last 3 gates AND at least 1.5x the median of its earlier
values. A gate script prints the block at its end; the exit code only reports a
malformed ledger.

  timing-breaches.py [--ledger PATH] [GATE]

GATE is `gate<N>` (a rerun letter is the same gate N); without it, every gate.
Exit 2 with `path:line: reason` on a malformed line, `path: reason` on a
missing or unreadable ledger, and on a bad GATE. The output grammar is in docs/Testing/TimingCritical.md.
"""

import argparse
import json
import math
import os
import re
import statistics
import sys
from collections import defaultdict
from pathlib import Path

KEYS = ("gate", "test", "quantity", "value", "budget", "unit", "load", "over")
UNITS = ("ms", "B")
GATE = re.compile(r"gate([1-9][0-9]*)[a-z]?")
TREND_RUN = 3
TREND_FACTOR = 1.5
HISTORY = 10
LAST = 5


class Malformed(Exception):
    pass


def ledger_path(arg):
    if arg:
        return Path(arg)
    if os.environ.get("HOLON_TIMING_MEASURE_LOG"):
        return Path(os.environ["HOLON_TIMING_MEASURE_LOG"])
    return Path(os.environ["HOME"]) / ".cache/holon-build/timing-checks.jsonl"


def gate_number(raw):
    """`local` -> None, `gate<N>[letter]` -> N; anything else raises."""
    if raw == "local":
        return None
    m = GATE.fullmatch(raw)
    if not m:
        raise Malformed(f"gate {raw!r} is not `local` or gate<N> with an optional rerun letter")
    return int(m.group(1))


def check_name(what, raw):
    if not isinstance(raw, str):
        raise Malformed(f"{what} is not a string")
    if raw == "" or raw.strip() != raw or any(ord(c) < 0x20 or 0x7F <= ord(c) <= 0x9F for c in raw):
        raise Malformed(f"{what} {raw!r} is empty, has edge whitespace or a control character")


def number(what, raw, signed=False):
    if isinstance(raw, bool) or not isinstance(raw, (int, float)):
        raise Malformed(f"{what} is not a number")
    try:
        value = float(raw)
    except OverflowError as e:
        raise Malformed(f"{what} is an integer beyond the float range") from e
    if not math.isfinite(value) or (value < 0 and not signed):
        raise Malformed(f"{what} {raw} is not a finite number >= 0")
    return value


def reject_constant(name):
    raise Malformed(f"{name} is not JSON")


def decode_line(raw):
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError as e:
        raise Malformed(f"not UTF-8: {e}") from e


def parse_line(text):
    try:
        obj = json.loads(text, parse_constant=reject_constant)
    except json.JSONDecodeError as e:
        raise Malformed(f"not JSON: {e}") from e
    if not isinstance(obj, dict):
        raise Malformed("not a JSON object")
    if set(obj) != set(KEYS):
        raise Malformed(f"keys {sorted(obj)} are not {sorted(KEYS)}")
    if not isinstance(obj["gate"], str):
        raise Malformed("gate is not a string")
    gate = gate_number(obj["gate"])
    check_name("test", obj["test"])
    check_name("quantity", obj["quantity"])
    if obj["unit"] not in UNITS:
        raise Malformed(f"unit {obj['unit']!r} is not one of {UNITS}")
    # A byte delta shrinks when a transition frees memory; a time never does.
    value = number("value", obj["value"], signed=obj["unit"] == "B")
    budget = number("budget", obj["budget"])
    load = number("load", obj["load"])
    if not isinstance(obj["over"], bool):
        raise Malformed("over is not a bool")
    if obj["over"] != (value > budget):
        raise Malformed(f"over={obj['over']} disagrees with value {value} > budget {budget}")
    return {
        "gate": gate,
        "test": obj["test"],
        "quantity": obj["quantity"],
        "value": value,
        "budget": budget,
        "unit": obj["unit"],
        "load": load,
        "over": obj["over"],
    }


def read_ledger(path):
    if not path.is_file():
        raise SystemExit(f"{path}: no ledger")
    try:
        raws = path.read_bytes().split(b"\n")
    except OSError as e:
        raise SystemExit(f"{path}: unreadable: {e.strerror}") from e
    if raws[-1] == b"":
        raws.pop()
    checks = []
    for n, raw in enumerate(raws, start=1):
        try:
            checks.append(parse_line(decode_line(raw)))
        except Malformed as e:
            raise SystemExit(f"{path}:{n}: {e}") from e
    return checks


def series_by_gate(checks):
    """(test, quantity) -> gate -> {"value": max, "over": any}."""
    series = defaultdict(dict)
    for c in checks:
        if c["gate"] is None:
            continue
        per_gate = series[(c["test"], c["quantity"])]
        g = per_gate.setdefault(c["gate"], {"value": c["value"], "over": False})
        g["value"] = max(g["value"], c["value"])
        g["over"] = g["over"] or c["over"]
    return series


def trend(per_gate, gate):
    """The median the gate's value is judged against, or None for no TREND."""
    ran = sorted(g for g in per_gate if g <= gate)
    if not ran or ran[-1] != gate or len(ran) < TREND_RUN:
        return None
    if not all(per_gate[g]["over"] for g in ran[-TREND_RUN:]):
        return None
    earlier = [per_gate[g]["value"] for g in ran[:-1][-HISTORY:]]
    median = statistics.median(earlier)
    if per_gate[gate]["value"] >= TREND_FACTOR * median:
        return median, ran[-TREND_RUN:]
    return None


def block(gate, checks, series):
    lines = [c for c in checks if c["gate"] == gate]
    over = [c for c in lines if c["over"]]
    out = [f"== timing gate{gate}: {len(over)} over of {len(lines)} checks =="]
    for c in over:
        per_gate = series[(c["test"], c["quantity"])]
        last = sorted(g for g in per_gate if g <= gate)[-LAST:]
        values = ",".join(repr(per_gate[g]["value"]) for g in last)
        out.append(
            "\t".join(
                ["over", c["test"], c["quantity"], repr(c["value"]), repr(c["budget"]),
                 c["unit"], repr(c["load"]), values]
            )
        )
    units = {(c["test"], c["quantity"]): c["unit"] for c in lines}
    for key in sorted(units):
        hit = trend(series[key], gate)
        if hit is None:
            continue
        median, gates = hit
        out.append(
            "\t".join(
                ["TREND", key[0], key[1], repr(series[key][gate]["value"]), repr(float(median)),
                 units[key], ",".join(f"gate{g}" for g in gates)]
            )
        )
    out.append(f"== end gate{gate} ==")
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--ledger")
    ap.add_argument("gate", nargs="?")
    args = ap.parse_args()
    if args.gate is not None:
        m = GATE.fullmatch(args.gate)
        if not m:
            raise SystemExit(f"GATE {args.gate!r} is not gate<N> with an optional rerun letter")
        gates = [int(m.group(1))]
    checks = read_ledger(ledger_path(args.ledger))
    if args.gate is None:
        gates = sorted({c["gate"] for c in checks if c["gate"] is not None})
    series = series_by_gate(checks)
    for gate in gates:
        print("\n".join(block(gate, checks, series)))


if __name__ == "__main__":
    try:
        main()
    except SystemExit as e:
        if isinstance(e.code, str):
            print(e.code, file=sys.stderr)
            sys.exit(2)
        raise
