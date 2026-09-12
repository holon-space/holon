#!/usr/bin/env python3
"""Decompose the burst arm's e2e latency into arrival, service and queue wait.

`stage="e2e"` is a wall clock from dispatch to the row landing in the reactive
mirror, so it charges every interaction for the ones queued ahead of it. Each
sample's dispatch instant is recoverable as `delivery_ts - ms`, which makes the
arrival process and the drain rate directly measurable from the same log.
"""
import re
import sys
from datetime import datetime

ANSI = re.compile(r"\x1b\[[0-9;]*m")
TS = re.compile(r"^(\d{4}-\d\d-\d\dT[\d:.]+)Z")
# UI-origin only: a `facade` sample (agent/MCP op) starts above the frontend
# dispatch seam, so it belongs to a different arrival process than the burst
# this script decomposes (D119.a).
E2E = re.compile(r'stage="e2e" action=set_field \S+ origin="ui" source="(\w+)" ms=(\d+)')
# The pre-D119.a shape: no `origin` between `block=` and `source=`. Disjoint
# from E2E above, so a match here means the log cannot be attributed at all.
E2E_LEGACY = re.compile(r'stage="e2e" action=set_field \S+ source="(\w+)" ms=(\d+)')
STAGE = re.compile(r'stage="(\w+)"[^\n]*? ms=(\d+)')


def load(path):
    ev, stages, legacy = [], {}, 0
    for line in open(path, errors="replace"):
        line = ANSI.sub("", line)
        t = TS.match(line)
        if not t:
            continue
        ts = datetime.strptime(t.group(1)[:26], "%Y-%m-%dT%H:%M:%S.%f").timestamp()
        m = E2E.search(line)
        if m:
            ev.append((ts, int(m.group(2))))
        elif E2E_LEGACY.search(line):
            legacy += 1
        s = STAGE.search(line)
        if s and s.group(1) not in ("e2e", "matview_ddl"):
            stages.setdefault(s.group(1), []).append(int(s.group(2)))
    if legacy:
        # Never report an empty decomposition from a log this parser cannot
        # attribute to a clock origin.
        raise SystemExit(
            f"{path}: {legacy} `stage=e2e` line(s) carry no `origin` field — this log "
            f"predates D119.a (2026-09-12) and its samples cannot be attributed to a "
            f"clock origin. Re-measure against a current build, or analyse it with a "
            f"pre-D119.a checkout. Refusing to report a partial population."
        )
    return ev, stages


def burst_window(ev):
    """The tightest 32-sample run: the burst arm delivers ~5ms apart, the paced
    arm ~90ms apart, so the densest window is unambiguously the burst."""
    best, span = None, None
    for i in range(len(ev) - 31):
        w = ev[i:i + 32]
        d = w[-1][0] - w[0][0]
        if span is None or d < span:
            best, span = w, d
    return best


def main(path, label, service_floor):
    ev, stages = load(path)
    w = burst_window(ev)
    dispatches = sorted(t - ms / 1000.0 for t, ms in w)
    deliveries = [t for t, _ in w]
    arr = [(dispatches[i + 1] - dispatches[i]) * 1000 for i in range(len(dispatches) - 1)]
    dlv = [(deliveries[i + 1] - deliveries[i]) * 1000 for i in range(len(deliveries) - 1)]
    mss = sorted(ms for _, ms in w)
    p50 = mss[len(mss) // 2]

    print(f"=== {label}  ({path})")
    print(f"  burst e2e ms, in delivery order : {[ms for _, ms in w]}")
    print(f"  arrivals spread over            : {(dispatches[-1]-dispatches[0])*1000:.0f} ms"
          f"  (mean interval {sum(arr)/len(arr):.1f} ms)")
    print(f"  deliveries spread over          : {(deliveries[-1]-deliveries[0])*1000:.0f} ms"
          f"  (mean interval {sum(dlv)/len(dlv):.1f} ms  = drain rate)")
    print(f"  burst p50 e2e                   : {p50} ms")
    print(f"  service floor (paced p50)       : {service_floor} ms")
    print(f"  queue wait = p50 - floor        : {p50 - service_floor} ms"
          f"  ({100.0*(p50-service_floor)/p50:.1f}% of the burst p50)")
    print("  named non-boot stages during the run:")
    for k, v in sorted(stages.items()):
        print(f"    {k:<28} n={len(v):<5} p50={sorted(v)[len(v)//2]} max={max(v)}")
    print()


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], int(sys.argv[3]))
