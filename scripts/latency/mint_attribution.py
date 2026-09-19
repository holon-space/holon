#!/usr/bin/env python3
"""Attribute the per-block materialized-view mints in a scale-gate run log.

The harness's `action_total` window closes when a transition's own dispatch and
settle finish, but a watch registration RETURNS BEFORE its view is minted: the
`CREATE MATERIALIZED VIEW` lands afterwards, inside whichever window happens to
be open next. So the stage that pays for the mint in `action_total` is not the
stage that caused it, and reading a `SetupWatch` rung as "the mint is cheap" is
a window artifact rather than a measurement.

This script scores the mints directly instead, on their own events, so the
scale gate can state what they cost without borrowing a window. It reports the
REPLAY-PHASE `watch_view_*` mints only, because the boot mints are a different
population paid once, before any navigation.

The phase boundary is the FIRST `action_total` — the moment the first replayed
transition closes — not the last `boot_ingest_total`. The org ingest finishes
before the UI layout does, so a boundary at `boot_ingest_total` files the
layout and sidebar watch registrations under "replay": measured on a
204-block log, 76 mints at mean 90.6 ms against 63 at mean 95.3 ms once the
layout watches are excluded. Those 13 are boot cost, they are cheap, and
counting them understates the per-navigation figure this gate reports.

    python3 scripts/latency/mint_attribution.py <run-log>

Fails loud on a log with no boot marker or no replay-phase mint: either means
the run did not exercise the path this gate exists to score, and a silent zero
would read exactly like a fixed tree.
"""

import re
import sys

ANSI = re.compile(r"\x1b\[[0-9;]*m")
MATVIEW = re.compile(r'stage="matview_ddl".*?view="(watch_view_[0-9a-f]+)".*?ms=(\d+)')
REPLAY_START = re.compile(r'stage="action_total"')


def percentile(values, p):
    """Nearest-rank percentile, matching the harness's own convention."""
    ordered = sorted(values)
    rank = max(1, min(len(ordered), int(len(ordered) * p + 0.9999)))
    return ordered[rank - 1]


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: mint_attribution.py <run-log>")
    path = sys.argv[1]
    with open(path, errors="replace") as fh:
        lines = [ANSI.sub("", line) for line in fh]

    boot_end = None
    for i, line in enumerate(lines):
        if REPLAY_START.search(line):
            boot_end = i
            break
    if boot_end is None:
        raise SystemExit(
            f"{path}: no `action_total` event — the workload never applied a transition, "
            f"so the boot and replay mints cannot be told apart")

    boot, replay = [], []
    for i, line in enumerate(lines):
        m = MATVIEW.search(line)
        if m:
            (boot if i < boot_end else replay).append(int(m.group(2)))

    if not replay:
        raise SystemExit(
            f"{path}: zero replay-phase watch-view mints. The workload navigates to "
            f"distinct never-visited blocks, so each one MUST mint a view; none means "
            f"the corpus did not drive the path this gate scores.")

    print("\n" + "=" * 72)
    print("PER-BLOCK VIEW MINTS  (watch_view_* DDL, scored on their own events)")
    print("=" * 72)
    print(f"{'phase':<12}{'n':>5}{'sum ms':>10}{'mean':>9}{'p50':>9}{'max':>9}")
    print("-" * 72)
    for label, vals in (("boot", boot), ("replay", replay)):
        if vals:
            print(f"{label:<12}{len(vals):>5}{sum(vals):>10}"
                  f"{sum(vals) / len(vals):>9.1f}{percentile(vals, 0.50):>9}{max(vals):>9}")
    print()
    print("The replay row is the scale cost. It is NOT inside the `total.SetupWatch`")
    print("rung: registration returns before the mint, so the DDL lands in the next")
    print("transition's window. That is why the `total.*` rungs are report-only.")


if __name__ == "__main__":
    main()
