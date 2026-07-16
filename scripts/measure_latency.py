#!/usr/bin/env python3
"""Parse target="holon_latency" tracing events into per-action latency tables.

The instrumentation (workstream W3) emits one greppable event per pipeline
stage, all under the `holon_latency` tracing target:

  stage=dispatch      action=<op>  block=<id>            ms=<dispatch->op-applied>
  stage=projection    ops=<n> blocks=<n> snapshot_ms=<n> ms=<full projection pass>
  stage=rows            source=<matview> rows=<n> seq=<n>  ms=<CDC batch apply>
  stage=e2e             action=<op> block=<id> source=<m>  ms=<dispatch->row visible (PROD)>
  stage=action_total    action=<kind>                      total_ms=<action->visible rows>
  stage=boot_feed_progress  present=<n> expected=<n>        (feed-convergence heartbeat; NO ms)

`action_total` is the end-to-end wall time of one UI action driven through the
REAL pipeline (dispatch -> Loro commit -> LoroProjection resample -> Turso/matview
CDC -> reactive row batch applied), measured by the headless composed harness.
It excludes only final GPU paint.

Usage:
    python3 scripts/measure_latency.py <logfile>
    <something> | python3 scripts/measure_latency.py -
    python3 scripts/measure_latency.py <logfile> --fail-over-p95 2000   # CI gate (exit 1 if worst action p95 > 2000ms)
"""
import re
import sys
from collections import defaultdict

FIELD = re.compile(r'(\w+)=(?:"([^"]*)"|(\S+))')
ANSI = re.compile(r'\x1b\[[0-9;]*m')


def parse(line):
    line = ANSI.sub("", line)
    if "holon_latency" not in line or "stage=" not in line:
        return None
    fields = {}
    for key, quoted, bare in FIELD.findall(line):
        fields[key] = quoted if quoted != "" or bare == "" else bare
    return fields if "stage" in fields else None


def pct(sorted_vals, p):
    if not sorted_vals:
        return 0.0
    k = (len(sorted_vals) - 1) * (p / 100.0)
    lo = int(k)
    hi = min(lo + 1, len(sorted_vals) - 1)
    return sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * (k - lo)


def stats(vals):
    s = sorted(vals)
    return (len(s), pct(s, 50), pct(s, 95), max(s) if s else 0.0,
            sum(s) / len(s) if s else 0.0)


def num(fields, key):
    try:
        return float(fields[key])
    except (KeyError, ValueError):
        return None


def main():
    args = sys.argv[1:]
    fail_over_p95 = None
    if "--fail-over-p95" in args:
        i = args.index("--fail-over-p95")
        fail_over_p95 = float(args[i + 1])
        del args[i:i + 2]
    if len(args) != 1:
        print(__doc__)
        sys.exit(2)
    src = sys.stdin if args[0] == "-" else open(args[0])

    total_by_action = defaultdict(list)
    e2e_by_action = defaultdict(list)
    dispatch_by_action = defaultdict(list)
    proj_ms, proj_snap, proj_blocks = [], [], []
    # mode=full|incremental counts, and per-reason breakdown of the full passes.
    # `reason` is a new field (Inc 0 of the reseed-latency workstream); logs from
    # before it landed have no `reason`, bucketed as "unlabeled" for back-compat.
    proj_mode_counts = defaultdict(int)
    full_reason_counts = defaultdict(int)
    rows_ms, rows_n = [], []
    # Boot ingest (and any other unrecognized stage): bucket generically by stage
    # name so new instrumentation surfaces without a script edit. `feed_timeouts`
    # counts feed barriers that hit their ceiling (caught_up=false) — i.e. how
    # much of the 2s per-file / 30s convergence budget actually binds.
    misc_by_stage = defaultdict(list)
    feed_timeouts = defaultdict(int)
    # boot_feed_progress is a convergence heartbeat with present/expected counts
    # and NO `ms` field, so the generic ms-bucket above silently drops it. Track
    # it explicitly: how many heartbeats fired and the furthest progress reached.
    feed_progress_ticks = 0
    feed_progress_max = (0, 0)  # (present, expected) at the furthest tick

    for line in src:
        f = parse(line)
        if not f:
            continue
        stage = f["stage"]
        if stage == "action_total":
            v = num(f, "total_ms")
            if v is not None:
                total_by_action[f.get("action", "?")].append(v)
        elif stage == "e2e":
            v = num(f, "ms")
            if v is not None:
                e2e_by_action[f.get("action", "?")].append(v)
        elif stage == "dispatch":
            v = num(f, "ms")
            if v is not None:
                dispatch_by_action[f.get("action", "?")].append(v)
        elif stage == "projection":
            mode = f.get("mode", "?")
            proj_mode_counts[mode] += 1
            if mode == "full":
                full_reason_counts[f.get("reason", "unlabeled")] += 1
            for lst, k in ((proj_ms, "ms"), (proj_snap, "snapshot_ms"),
                           (proj_blocks, "blocks")):
                v = num(f, k)
                if v is not None:
                    lst.append(v)
        elif stage == "rows":
            for lst, k in ((rows_ms, "ms"), (rows_n, "rows")):
                v = num(f, k)
                if v is not None:
                    lst.append(v)
        elif stage == "boot_feed_progress":
            feed_progress_ticks += 1
            present, expected = num(f, "present"), num(f, "expected")
            if present is not None and present >= feed_progress_max[0]:
                feed_progress_max = (present, expected or feed_progress_max[1])
        else:
            # Generic bucket — boot_parse / boot_write / boot_feed_wait /
            # boot_place_wait / boot_file / boot_ingest_total / boot_feed_converge
            # and any future stage.
            v = num(f, "ms")
            if v is not None:
                misc_by_stage[stage].append(v)
            if f.get("caught_up") == "false":
                feed_timeouts[stage] += 1

    def table(title, by_key, unit="ms"):
        print(f"\n== {title} ==")
        print(f"{'action':<22}{'n':>6}{'p50':>9}{'p95':>9}{'max':>9}{'mean':>9}  ({unit})")
        print("-" * 72)
        for k in sorted(by_key, key=lambda x: -sum(by_key[x])):
            n, p50, p95, mx, mean = stats(by_key[k])
            print(f"{k:<22}{n:>6}{p50:>9.1f}{p95:>9.1f}{mx:>9.1f}{mean:>9.1f}")

    print("=" * 72)
    print("HOLON UI ACTION LATENCY  (headless composed pipeline; excludes GPU paint)")
    print("=" * 72)

    if total_by_action:
        table("END-TO-END  action -> visible rows  (stage=action_total)", total_by_action)
    else:
        print("\n(no stage=action_total events - was RUST_LOG=holon_latency=debug set?)")

    if e2e_by_action:
        table("PROD END-TO-END  interaction -> visible  (stage=e2e)", e2e_by_action)

    if dispatch_by_action:
        table("DISPATCH stage  action -> op applied  (stage=dispatch)", dispatch_by_action)

    print("\n== PIPELINE STAGE COST (global, all actions) ==")
    print(f"{'stage':<28}{'n':>6}{'p50':>9}{'p95':>9}{'max':>9}{'mean':>9}  (ms)")
    print("-" * 72)
    for name, vals in (("projection (full pass)", proj_ms),
                       ("projection (snapshot only)", proj_snap),
                       ("rows (CDC batch apply)", rows_ms)):
        if vals:
            n, p50, p95, mx, mean = stats(vals)
            print(f"{name:<28}{n:>6}{p50:>9.1f}{p95:>9.1f}{mx:>9.1f}{mean:>9.1f}")

    if proj_mode_counts:
        full_n = proj_mode_counts.get("full", 0)
        incr_n = proj_mode_counts.get("incremental", 0)
        print("\n== PROJECTION MODE ATTRIBUTION ==")
        print(f"  incremental (O(changed) fast path): {incr_n}")
        print(f"  full (reseed walk):                 {full_n}")
        # Legitimate seeds vs the four reseed leaks vs sink-fail recovery.
        legit = {"coldboot"}
        leaks = {"empty_pending_moved_frontier", "unsettled", "orphan",
                 "oversized"}
        if full_reason_counts:
            print("  full-pass reasons:")
            for reason in sorted(full_reason_counts,
                                 key=lambda r: -full_reason_counts[r]):
                cnt = full_reason_counts[reason]
                tag = ("seed" if reason in legit else
                       "LEAK" if reason in leaks else
                       "recovery" if reason == "sink_fail" else "?")
                print(f"    {reason:<32}{cnt:>6}  [{tag}]")

    if misc_by_stage:
        print("\n== BOOT INGEST (stage=boot_*, per file unless noted) ==")
        print(f"{'stage':<28}{'n':>6}{'p50':>9}{'p95':>9}{'max':>9}{'mean':>9}  (ms)")
        print("-" * 72)
        for name in sorted(misc_by_stage, key=lambda x: -sum(misc_by_stage[x])):
            n, p50, p95, mx, mean = stats(misc_by_stage[name])
            print(f"{name:<28}{n:>6}{p50:>9.1f}{p95:>9.1f}{mx:>9.1f}{mean:>9.1f}")
        if feed_timeouts:
            for name, cnt in sorted(feed_timeouts.items()):
                print(f"  ! {name}: {cnt} occurrence(s) hit the feed ceiling "
                      f"(caught_up=false)")
        else:
            print("  (all feed barriers converged; caught_up=true everywhere)")

    if feed_progress_ticks:
        pres, exp = feed_progress_max
        print(f"\nfeed convergence: {feed_progress_ticks} progress heartbeat(s); "
              f"furthest {pres:.0f}/{exp:.0f} blocks present "
              f"(stage=boot_feed_progress — heartbeat, no per-tick ms)")

    if proj_blocks:
        print(f"\nprojection doc size: blocks p50={pct(sorted(proj_blocks),50):.0f} "
              f"max={max(proj_blocks):.0f}  (full-document DFS snapshot per commit)")
    if rows_n:
        print(f"rows per CDC batch:  p50={pct(sorted(rows_n),50):.0f} "
              f"max={max(rows_n):.0f}")

    if proj_ms and total_by_action:
        all_totals = [v for vs in total_by_action.values() for v in vs]
        tot_sum = sum(all_totals)
        proj_sum = sum(proj_ms)
        print(f"\nDOMINATOR: projection accounts for {proj_sum:.0f}ms across "
              f"{len(proj_ms)} passes vs {tot_sum:.0f}ms of end-to-end action wall "
              f"({100*proj_sum/tot_sum:.0f}% of total action time).")

    # CI gate: fail if any action's end-to-end p95 exceeds the threshold. The gate
    # is intentionally generous (the headless keystone runs small docs); it exists to
    # catch a gross latency regression, not to police micro-fluctuations.
    if fail_over_p95 is not None:
        if not total_by_action:
            print(f"\nLATENCY GATE: no stage=action_total events found - cannot "
                  f"evaluate p95 threshold. Was RUST_LOG=holon_latency=debug set?",
                  file=sys.stderr)
            sys.exit(3)
        worst_action, worst_p95 = None, -1.0
        for action, vals in total_by_action.items():
            _, _, p95, _, _ = stats(vals)
            if p95 > worst_p95:
                worst_action, worst_p95 = action, p95
        print(f"\nLATENCY GATE: worst end-to-end p95 = {worst_p95:.1f}ms "
              f"(action={worst_action!r}); threshold = {fail_over_p95:.1f}ms")
        if worst_p95 > fail_over_p95:
            print(f"LATENCY GATE FAILED: p95 {worst_p95:.1f}ms > {fail_over_p95:.1f}ms",
                  file=sys.stderr)
            sys.exit(1)
        print("LATENCY GATE PASSED")


if __name__ == "__main__":
    main()
