#!/usr/bin/env python3
"""Summarize a tracing-chrome JSON trace.

Designed for the holon PBT runs (TraceStyle::Async). Highlights:
  - wall-clock vs CPU-active time per thread (idle-gap detection)
  - top spans by total/self/wall-occupied duration
  - spans whose names match wait/sleep/poll patterns (suspected idle sources)
  - largest idle gaps on the main PBT thread

Usage:
  scripts/analyze-chrome-trace.py <trace.json> [--top N] [--thread <substring>]

The trace file may be either a bare JSON array of events or
{"traceEvents": [...]}. tracing-chrome with TraceStyle::Async emits
"b"/"e" (async begin/end) events keyed by (id, name) plus "i" instants.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from dataclasses import dataclass, field


IDLE_NAME_PATTERNS = re.compile(
    r"(wait|sleep|park|poll|recv|timeout|settle|quiesc|idle)",
    re.IGNORECASE,
)


@dataclass
class Span:
    name: str
    cat: str
    tid: int
    start_us: int
    end_us: int

    @property
    def dur_us(self) -> int:
        return self.end_us - self.start_us


def load_events(path: str) -> list[dict]:
    with open(path) as f:
        text = f.read()
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        # tracing-chrome leaves the file truncated (no closing `]`) whenever the
        # process exits without dropping the `FlushGuard` — which is every PASSING
        # test, since the guard lives in a `OnceLock` (see test_tracing.rs). The
        # writer emits one JSON object per line after the opening `[`, so recover by
        # parsing line-by-line and discarding the final partial line.
        data = []
        for line in text.splitlines():
            line = line.strip().rstrip(",")
            if not line or line in ("[", "]"):
                continue
            if line.startswith("["):
                line = line[1:]
            try:
                data.append(json.loads(line))
            except json.JSONDecodeError:
                continue  # trailing partial line
        print(f"[analyze-chrome-trace] recovered {len(data)} events from truncated "
              f"trace (unflushed FlushGuard — pass CHROME_TRACE flush or ignore)",
              file=sys.stderr)
    if isinstance(data, dict):
        return data.get("traceEvents", [])
    return data


def pair_async_events(events: list[dict]) -> list[Span]:
    """Match async 'b'/'e' pairs by (name, id, tid). Also handles 'X' (complete) events."""
    open_starts: dict[tuple, list[dict]] = defaultdict(list)
    spans: list[Span] = []
    for ev in events:
        ph = ev.get("ph")
        if ph == "X":
            spans.append(Span(
                name=ev.get("name", "?"),
                cat=ev.get("cat", ""),
                tid=ev.get("tid", 0),
                start_us=int(ev["ts"]),
                end_us=int(ev["ts"]) + int(ev.get("dur", 0)),
            ))
        elif ph in ("b", "B"):
            key = (ev.get("name"), ev.get("id"), ev.get("tid"))
            open_starts[key].append(ev)
        elif ph in ("e", "E"):
            key = (ev.get("name"), ev.get("id"), ev.get("tid"))
            stack = open_starts.get(key)
            if stack:
                start_ev = stack.pop()
                spans.append(Span(
                    name=ev.get("name", "?"),
                    cat=ev.get("cat", ""),
                    tid=ev.get("tid", 0),
                    start_us=int(start_ev["ts"]),
                    end_us=int(ev["ts"]),
                ))
    return spans


def thread_names(events: list[dict]) -> dict[int, str]:
    names: dict[int, str] = {}
    for ev in events:
        if ev.get("name") == "thread_name" and ev.get("ph") == "M":
            tid = ev.get("tid", 0)
            names[tid] = ev.get("args", {}).get("name", f"tid-{tid}")
    return names


def cpu_active_us(spans: list[Span]) -> int:
    """Merge overlapping intervals (within one trace) to get wall-occupied time."""
    if not spans:
        return 0
    intervals = sorted((s.start_us, s.end_us) for s in spans)
    merged = [intervals[0]]
    for start, end in intervals[1:]:
        last_start, last_end = merged[-1]
        if start <= last_end:
            merged[-1] = (last_start, max(last_end, end))
        else:
            merged.append((start, end))
    return sum(end - start for start, end in merged)


def largest_gaps(spans: list[Span], top: int) -> list[tuple[int, int, int, str, str]]:
    """Return (gap_us, gap_start, gap_end, prev_name, next_name) for top N idle gaps."""
    if not spans:
        return []
    intervals = sorted(((s.start_us, s.end_us, s.name) for s in spans), key=lambda x: x[0])
    merged: list[tuple[int, int, str, str]] = []
    cur_start, cur_end, first_name, last_name = intervals[0][0], intervals[0][1], intervals[0][2], intervals[0][2]
    for start, end, name in intervals[1:]:
        if start <= cur_end:
            cur_end = max(cur_end, end)
            last_name = name
        else:
            merged.append((cur_start, cur_end, first_name, last_name))
            cur_start, cur_end, first_name, last_name = start, end, name, name
    merged.append((cur_start, cur_end, first_name, last_name))

    gaps = []
    for i in range(1, len(merged)):
        prev_start, prev_end, _, prev_last = merged[i - 1]
        nxt_start, nxt_end, nxt_first, _ = merged[i]
        gap = nxt_start - prev_end
        gaps.append((gap, prev_end, nxt_start, prev_last, nxt_first))
    gaps.sort(key=lambda g: -g[0])
    return gaps[:top]


def fmt_us(us: int) -> str:
    if us < 1_000:
        return f"{us}us"
    if us < 1_000_000:
        return f"{us / 1_000:.1f}ms"
    return f"{us / 1_000_000:.2f}s"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("trace", help="path to trace.json")
    parser.add_argument("--top", type=int, default=20)
    parser.add_argument("--thread", default=None,
                        help="restrict gap analysis to thread whose name contains this substring")
    args = parser.parse_args()

    events = load_events(args.trace)
    tnames = thread_names(events)
    spans = pair_async_events(events)
    if not spans:
        print("no spans found (chrome-trace produced no b/e/X events)", file=sys.stderr)
        return 1

    wall_start = min(s.start_us for s in spans)
    wall_end = max(s.end_us for s in spans)
    wall = wall_end - wall_start
    print(f"trace wall time: {fmt_us(wall)}  ({len(spans)} spans across {len(tnames) or '?'} threads)")
    print()

    # Per-thread occupancy
    print("== per-thread CPU-active vs wall ==")
    by_tid: dict[int, list[Span]] = defaultdict(list)
    for s in spans:
        by_tid[s.tid].append(s)
    rows = []
    for tid, ts in by_tid.items():
        active = cpu_active_us(ts)
        rows.append((active, tid, len(ts)))
    rows.sort(reverse=True)
    for active, tid, count in rows[:10]:
        name = tnames.get(tid, f"tid-{tid}")
        pct = active * 100 / wall if wall else 0
        print(f"  {name[:40]:<40}  active={fmt_us(active):>8}  ({pct:5.1f}%)  spans={count}")
    print()

    # Top spans by total duration (own time across all instances)
    by_name_total: dict[str, int] = defaultdict(int)
    by_name_count: dict[str, int] = defaultdict(int)
    by_name_max: dict[str, int] = defaultdict(int)
    for s in spans:
        by_name_total[s.name] += s.dur_us
        by_name_count[s.name] += 1
        by_name_max[s.name] = max(by_name_max[s.name], s.dur_us)

    print(f"== top {args.top} spans by total duration ==")
    ranked = sorted(by_name_total.items(), key=lambda kv: -kv[1])[:args.top]
    for name, total in ranked:
        cnt = by_name_count[name]
        mx = by_name_max[name]
        print(f"  {fmt_us(total):>8}  n={cnt:<6} max={fmt_us(mx):>8}  {name}")
    print()

    # Idle-suspect spans (name matches wait/sleep/poll patterns)
    print(f"== idle-suspect spans (name matches wait|sleep|park|poll|recv|timeout|settle|quiesc|idle) ==")
    suspect = [(n, t, by_name_count[n], by_name_max[n])
               for n, t in by_name_total.items()
               if IDLE_NAME_PATTERNS.search(n)]
    suspect.sort(key=lambda x: -x[1])
    for name, total, cnt, mx in suspect[:args.top]:
        print(f"  {fmt_us(total):>8}  n={cnt:<6} max={fmt_us(mx):>8}  {name}")
    print()

    # Largest idle gaps — by thread (or filtered)
    selected_tids = list(by_tid.keys())
    if args.thread:
        selected_tids = [tid for tid in selected_tids
                         if args.thread.lower() in tnames.get(tid, "").lower()]
    print(f"== largest idle gaps per thread (top {args.top} per thread) ==")
    for tid in selected_tids:
        name = tnames.get(tid, f"tid-{tid}")
        gaps = largest_gaps(by_tid[tid], args.top)
        if not gaps or gaps[0][0] < 1_000:
            continue
        print(f"  thread {name}:")
        for gap, gstart, gend, prev_n, next_n in gaps:
            if gap < 1_000:
                break
            t_rel = (gstart - wall_start) / 1_000_000
            print(f"    gap={fmt_us(gap):>8}  @ t={t_rel:6.2f}s   prev='{prev_n[:40]}' -> next='{next_n[:40]}'")
    return 0


if __name__ == "__main__":
    sys.exit(main())
