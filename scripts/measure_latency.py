#!/usr/bin/env python3
"""Parse target="holon_latency" tracing events into per-action latency tables.

The instrumentation (workstream W3) emits one greppable event per pipeline
stage, all under the `holon_latency` tracing target:

  stage=dispatch      action=<op>  block=<id>            ms=<dispatch->op-applied>
  stage=projection    ops=<n> blocks=<n> snapshot_ms=<n> ms=<full projection pass>
  stage=rows          source=<matview> rows=<n> seq=<n>  ms=<CDC batch apply>
  stage=action_total  action=<kind>                      total_ms=<action->visible rows>

`action_total` is the end-to-end wall time of one UI action driven through the
REAL pipeline (dispatch -> Loro commit -> LoroProjection resample -> Turso/matview
CDC -> reactive row batch applied), measured by the headless composed harness.
It excludes only final GPU paint.

Usage:
    python3 scripts/measure_latency.py <logfile>
    <something> | python3 scripts/measure_latency.py -
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
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    src = sys.stdin if sys.argv[1] == "-" else open(sys.argv[1])

    total_by_action = defaultdict(list)
    dispatch_by_action = defaultdict(list)
    proj_ms, proj_snap, proj_blocks = [], [], []
    rows_ms, rows_n = [], []

    for line in src:
        f = parse(line)
        if not f:
            continue
        stage = f["stage"]
        if stage == "action_total":
            v = num(f, "total_ms")
            if v is not None:
                total_by_action[f.get("action", "?")].append(v)
        elif stage == "dispatch":
            v = num(f, "ms")
            if v is not None:
                dispatch_by_action[f.get("action", "?")].append(v)
        elif stage == "projection":
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


if __name__ == "__main__":
    main()
