#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "pm4py>=2.7",
#     "pandas>=2.0",
# ]
# ///
"""
Process mining analysis of holon tracing logs using PM4Py.

Parses structured tracing output (timestamp LEVEL module: [Component] message),
groups events into cases (sync cycles, startup, UI watch sessions), discovers
process models, and reports timing/bottleneck insights.

Usage:
    uv run scripts/analyze-log-pm4py.py /tmp/holon.log
    uv run scripts/analyze-log-pm4py.py /tmp/holon.log --case-strategy component
    uv run scripts/analyze-log-pm4py.py /tmp/holon.log --export-csv /tmp/event_log.csv
"""

import argparse
import re
import sys
from collections import defaultdict
from datetime import datetime, timedelta
from pathlib import Path

import pandas as pd
import pm4py


TIMESTAMP_RE = re.compile(
    r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z)\s+"
    r"(TRACE|DEBUG|INFO|WARN|ERROR)\s+"
    r"([\w:]+):\s+"
    r"(?:\[([^\]]+)\]\s*)?"
    r"(.*)"
)

# Events that mark the start of a new "case" (sync cycle, startup, etc.)
CASE_BOUNDARIES = {
    "sync_cycle": [
        r"Re-syncing entity",
        r"Full sync diff",
    ],
    "startup": [
        r"Starting .* frontend",
        r"Turso database opened",
        r"Creating core tables",
    ],
    "ui_watch": [
        r"UiWatcher.*new watch",
        r"UiWatcher.*structural change",
    ],
    "org_sync": [
        r"OrgSyncController.*file changed",
        r"OrgSyncController.*re-render",
    ],
}


def parse_log_line(line: str) -> dict | None:
    m = TIMESTAMP_RE.match(line.strip())
    if not m:
        return None
    ts_str, level, module, component, message = m.groups()
    ts = datetime.fromisoformat(ts_str.replace("Z", "+00:00"))
    activity = f"[{component}] {message[:80]}" if component else f"{module.split('::')[-1]}: {message[:80]}"
    return {
        "timestamp": ts,
        "level": level,
        "module": module,
        "component": component or module.split("::")[-1],
        "message": message.strip(),
        "activity": activity,
    }


def assign_cases_by_component(events: list[dict]) -> list[dict]:
    """Each unique component gets its own case; sequential numbering within component."""
    counters: dict[str, int] = defaultdict(int)
    last_ts: dict[str, datetime] = {}
    gap_threshold = timedelta(seconds=5)

    for ev in events:
        comp = ev["component"]
        ts = ev["timestamp"]
        if comp in last_ts and (ts - last_ts[comp]) > gap_threshold:
            counters[comp] += 1
        last_ts[comp] = ts
        ev["case_id"] = f"{comp}#{counters[comp]}"
    return events


def assign_cases_by_time_window(events: list[dict], window_sec: float = 2.0) -> list[dict]:
    """Group events into cases by time proximity."""
    if not events:
        return events
    case_id = 0
    last_ts = events[0]["timestamp"]
    window = timedelta(seconds=window_sec)

    for ev in events:
        if (ev["timestamp"] - last_ts) > window:
            case_id += 1
        last_ts = ev["timestamp"]
        ev["case_id"] = f"window#{case_id}"
    return events


def assign_cases_by_sync_cycle(events: list[dict]) -> list[dict]:
    """Specifically track MCP sync cycles as cases."""
    cycle_id = 0
    in_cycle = False

    for ev in events:
        msg = ev["message"]
        if "Re-syncing entity" in msg:
            cycle_id += 1
            in_cycle = True
        elif "Full sync diff" in msg:
            in_cycle = False

        if in_cycle or "sync" in msg.lower():
            ev["case_id"] = f"sync#{cycle_id}"
        else:
            ev["case_id"] = f"other#{ev['component']}"
    return events


STRATEGIES = {
    "component": assign_cases_by_component,
    "time_window": assign_cases_by_time_window,
    "sync_cycle": assign_cases_by_sync_cycle,
}


def build_event_log(events: list[dict]) -> pd.DataFrame:
    rows = []
    for ev in events:
        rows.append({
            "case:concept:name": ev["case_id"],
            "concept:name": ev["activity"],
            "time:timestamp": ev["timestamp"],
            "level": ev["level"],
            "module": ev["module"],
            "component": ev["component"],
        })
    return pd.DataFrame(rows)


def print_section(title: str):
    print(f"\n{'='*70}")
    print(f"  {title}")
    print(f"{'='*70}\n")


def analyze_timing(events: list[dict]):
    """Find slow gaps between consecutive events."""
    print_section("TIMING ANALYSIS — Largest Gaps Between Events")

    gaps = []
    for i in range(1, len(events)):
        delta = (events[i]["timestamp"] - events[i - 1]["timestamp"]).total_seconds()
        if delta > 0.5:
            gaps.append((delta, events[i - 1], events[i]))

    gaps.sort(key=lambda x: -x[0])
    for delta, before, after in gaps[:15]:
        print(f"  {delta:8.3f}s  gap")
        print(f"    before: {before['timestamp'].strftime('%H:%M:%S.%f')} {before['activity'][:90]}")
        print(f"    after:  {after['timestamp'].strftime('%H:%M:%S.%f')} {after['activity'][:90]}")
        print()


def analyze_component_durations(events: list[dict]):
    """Total time spent per component."""
    print_section("COMPONENT ACTIVITY DURATION")

    comp_events: dict[str, list] = defaultdict(list)
    for ev in events:
        comp_events[ev["component"]].append(ev["timestamp"])

    durations = []
    for comp, timestamps in comp_events.items():
        if len(timestamps) >= 2:
            span = (max(timestamps) - min(timestamps)).total_seconds()
            durations.append((span, comp, len(timestamps)))

    durations.sort(key=lambda x: -x[0])
    print(f"  {'Component':<40} {'Duration':>10} {'Events':>8}")
    print(f"  {'-'*40} {'-'*10} {'-'*8}")
    for span, comp, count in durations[:20]:
        print(f"  {comp:<40} {span:>9.2f}s {count:>8}")


def analyze_transaction_latency(events: list[dict]):
    """Measure BEGIN→COMMIT transaction times."""
    print_section("TRANSACTION LATENCY (BEGIN → COMMIT)")

    pending_tx = None
    tx_times = []

    for ev in events:
        if "actor_tx_begin" in ev["message"]:
            pending_tx = ev["timestamp"]
        elif "actor_tx_commit" in ev["message"] and pending_tx:
            delta = (ev["timestamp"] - pending_tx).total_seconds()
            tx_times.append(delta)
            pending_tx = None

    if not tx_times:
        print("  No transactions found.")
        return

    tx_times.sort(reverse=True)
    print(f"  Total transactions: {len(tx_times)}")
    print(f"  Mean:   {sum(tx_times)/len(tx_times)*1000:.1f}ms")
    print(f"  Median: {tx_times[len(tx_times)//2]*1000:.1f}ms")
    print(f"  P95:    {tx_times[int(len(tx_times)*0.05)]*1000:.1f}ms")
    print(f"  Max:    {tx_times[0]*1000:.1f}ms")
    print(f"\n  Slowest 5 transactions (ms):")
    for t in tx_times[:5]:
        print(f"    {t*1000:.1f}ms")


def analyze_sync_cycles(events: list[dict]):
    """Track MCP sync cycle durations and record counts."""
    print_section("MCP SYNC CYCLE ANALYSIS")

    cycles = []
    current_start = None
    current_entity = None

    for ev in events:
        msg = ev["message"]
        if "Re-syncing entity" in msg:
            m = re.search(r"entity '(\w+)'", msg)
            current_entity = m.group(1) if m else "unknown"
            current_start = ev["timestamp"]
        elif "Full sync diff" in msg and current_start:
            duration = (ev["timestamp"] - current_start).total_seconds()
            m = re.search(r"(\d+) new, (\d+) updated, (\d+) removed, (\d+) unchanged", msg)
            stats = m.groups() if m else ("?", "?", "?", "?")
            cycles.append({
                "entity": current_entity,
                "duration": duration,
                "new": stats[0],
                "updated": stats[1],
                "removed": stats[2],
                "unchanged": stats[3],
            })
            current_start = None
        elif "Failed to resync" in msg:
            if current_start:
                duration = (ev["timestamp"] - current_start).total_seconds()
                cycles.append({
                    "entity": current_entity or "unknown",
                    "duration": duration,
                    "new": "FAIL",
                    "updated": "FAIL",
                    "removed": "FAIL",
                    "unchanged": "FAIL",
                })
                current_start = None

    if not cycles:
        print("  No sync cycles found.")
        return

    print(f"  {'Entity':<12} {'Duration':>10} {'New':>6} {'Upd':>6} {'Rem':>6} {'Unch':>8}")
    print(f"  {'-'*12} {'-'*10} {'-'*6} {'-'*6} {'-'*6} {'-'*8}")
    for c in cycles:
        print(f"  {c['entity']:<12} {c['duration']:>9.2f}s {c['new']:>6} {c['updated']:>6} {c['removed']:>6} {c['unchanged']:>8}")

    successful = [c for c in cycles if c["new"] != "FAIL"]
    failed = [c for c in cycles if c["new"] == "FAIL"]
    if successful:
        by_entity = defaultdict(list)
        for c in successful:
            by_entity[c["entity"]].append(c["duration"])
        print(f"\n  Summary by entity:")
        for entity, durs in sorted(by_entity.items()):
            print(f"    {entity:<12} avg={sum(durs)/len(durs):.2f}s  max={max(durs):.2f}s  count={len(durs)}")
    if failed:
        print(f"\n  Failed syncs: {len(failed)}")


def run_process_discovery(df: pd.DataFrame):
    """Run PM4Py process discovery and print results."""
    print_section("PROCESS DISCOVERY (Inductive Miner)")

    df = pm4py.format_dataframe(df, case_id="case:concept:name", activity_key="concept:name", timestamp_key="time:timestamp")
    event_log = pm4py.convert_to_event_log(df)

    # Discover process model
    net, im, fm = pm4py.discover_petri_net_inductive(event_log)

    # Conformance checking
    fitness = pm4py.fitness_token_based_replay(event_log, net, im, fm)
    print(f"  Token-based replay fitness:")
    print(f"    Average trace fitness: {fitness.get('average_trace_fitness', 'N/A'):.4f}")
    print(f"    Percentage fit traces: {fitness.get('percentage_of_fitting_traces', 'N/A'):.1f}%")

    # Start/end activities
    start_activities = pm4py.get_start_activities(event_log)
    end_activities = pm4py.get_end_activities(event_log)
    print(f"\n  Top start activities:")
    for act, count in sorted(start_activities.items(), key=lambda x: -x[1])[:10]:
        print(f"    {count:>4}x  {act[:90]}")
    print(f"\n  Top end activities:")
    for act, count in sorted(end_activities.items(), key=lambda x: -x[1])[:10]:
        print(f"    {count:>4}x  {act[:90]}")

    # Case durations
    case_durations = pm4py.get_all_case_durations(event_log)
    if case_durations:
        print(f"\n  Case duration statistics:")
        print(f"    Cases:  {len(case_durations)}")
        print(f"    Mean:   {sum(case_durations)/len(case_durations):.2f}s")
        print(f"    Max:    {max(case_durations):.2f}s")
        print(f"    Min:    {min(case_durations):.2f}s")

    # Variants (unique process execution patterns)
    variants = pm4py.get_variants(event_log)
    print(f"\n  Process variants (unique execution patterns): {len(variants)}")
    print(f"  Top 10 most common:")
    sorted_variants = sorted(variants.items(), key=lambda x: -len(x[1]))
    for variant_tuple, traces in sorted_variants[:10]:
        activities = list(variant_tuple) if isinstance(variant_tuple, tuple) else [variant_tuple]
        short = " → ".join(str(a)[:40] for a in activities[:5])
        if len(activities) > 5:
            short += f" → ... ({len(activities)} steps)"
        print(f"    {len(traces):>4}x  {short}")


def analyze_level_distribution(events: list[dict]):
    print_section("LOG LEVEL DISTRIBUTION")
    counts = defaultdict(int)
    for ev in events:
        counts[ev["level"]] += 1
    total = len(events)
    for level in ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"]:
        c = counts.get(level, 0)
        pct = c / total * 100 if total else 0
        bar = "#" * int(pct / 2)
        print(f"  {level:<6} {c:>6} ({pct:5.1f}%)  {bar}")


def analyze_errors_and_warnings(events: list[dict]):
    print_section("WARNINGS AND ERRORS")
    problems = [ev for ev in events if ev["level"] in ("WARN", "ERROR")]
    if not problems:
        print("  No warnings or errors found.")
        return

    # Group by message template (first 60 chars)
    templates = defaultdict(list)
    for ev in problems:
        key = f"[{ev['level']}] {ev['component']}: {ev['message'][:60]}"
        templates[key].append(ev["timestamp"])

    for key, timestamps in sorted(templates.items(), key=lambda x: -len(x[1])):
        first = min(timestamps).strftime("%H:%M:%S")
        last = max(timestamps).strftime("%H:%M:%S")
        print(f"  {len(timestamps):>4}x  {key}")
        if len(timestamps) > 1:
            print(f"         first={first}  last={last}")


def main():
    parser = argparse.ArgumentParser(description="Process mining analysis of holon logs")
    parser.add_argument("logfile", type=Path, help="Path to the log file")
    parser.add_argument("--case-strategy", choices=list(STRATEGIES.keys()), default="component",
                        help="How to group events into cases (default: component)")
    parser.add_argument("--min-level", choices=["TRACE", "DEBUG", "INFO", "WARN", "ERROR"], default="INFO",
                        help="Minimum log level to include (default: INFO)")
    parser.add_argument("--export-csv", type=Path, help="Export event log as CSV for external tools")
    args = parser.parse_args()

    level_order = {"TRACE": 0, "DEBUG": 1, "INFO": 2, "WARN": 3, "ERROR": 4}
    min_level = level_order[args.min_level]

    print(f"Parsing {args.logfile} ...")
    lines = args.logfile.read_text().splitlines()
    events = []
    for line in lines:
        ev = parse_log_line(line)
        if ev and level_order.get(ev["level"], 0) >= min_level:
            events.append(ev)

    print(f"Parsed {len(events)} events from {len(lines)} lines (min_level={args.min_level})")

    if not events:
        print("No events to analyze.")
        sys.exit(1)

    time_range = events[-1]["timestamp"] - events[0]["timestamp"]
    print(f"Time range: {events[0]['timestamp'].strftime('%H:%M:%S')} → {events[-1]['timestamp'].strftime('%H:%M:%S')} ({time_range.total_seconds():.0f}s)")

    # Domain-specific analyses (no case assignment needed)
    analyze_level_distribution(events)
    analyze_errors_and_warnings(events)
    analyze_timing(events)
    analyze_component_durations(events)
    analyze_transaction_latency(events)
    analyze_sync_cycles(events)

    # Process mining with case assignment
    strategy_fn = STRATEGIES[args.case_strategy]
    events = strategy_fn(events)

    df = build_event_log(events)

    if args.export_csv:
        df.to_csv(args.export_csv, index=False)
        print(f"\nExported {len(df)} events to {args.export_csv}")

    run_process_discovery(df)


if __name__ == "__main__":
    main()
