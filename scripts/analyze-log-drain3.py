#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "drain3>=0.9",
# ]
# ///
"""
Log template mining and anomaly detection using Drain3.

Clusters log lines into templates (patterns), reports frequency statistics,
identifies rare/anomalous patterns, and detects temporal anomalies.

Usage:
    uv run scripts/analyze-log-drain3.py /tmp/holon.log
    uv run scripts/analyze-log-drain3.py /tmp/holon.log --top 30
    uv run scripts/analyze-log-drain3.py /tmp/holon.log --anomaly-threshold 2
    uv run scripts/analyze-log-drain3.py /tmp/holon.log --show-rare
"""

import argparse
import re
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path

from drain3 import TemplateMiner
from drain3.template_miner_config import TemplateMinerConfig


TIMESTAMP_RE = re.compile(
    r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z)\s+"
    r"(TRACE|DEBUG|INFO|WARN|ERROR)\s+"
    r"([\w:]+):\s+"
    r"(.*)"
)


def parse_line(line: str) -> dict | None:
    m = TIMESTAMP_RE.match(line.strip())
    if not m:
        return None
    ts_str, level, module, message = m.groups()
    ts = datetime.fromisoformat(ts_str.replace("Z", "+00:00"))
    return {
        "timestamp": ts,
        "level": level,
        "module": module,
        "message": message.strip(),
        "raw": line.strip(),
    }


def create_template_miner() -> TemplateMiner:
    config = TemplateMinerConfig()
    config.drain_sim_th = 0.4
    config.drain_depth = 4
    config.drain_max_children = 100
    config.drain_max_clusters = 1024
    config.profiling_enabled = False
    return TemplateMiner(config=config)


def print_section(title: str):
    print(f"\n{'='*70}")
    print(f"  {title}")
    print(f"{'='*70}\n")


def analyze_templates(miner: TemplateMiner, top_n: int):
    print_section("LOG TEMPLATES BY FREQUENCY")

    clusters = sorted(miner.drain.clusters, key=lambda c: -c.size)
    total = sum(c.size for c in clusters)

    print(f"  Total lines clustered: {total}")
    print(f"  Unique templates: {len(clusters)}")
    print(f"  Compression ratio: {total/max(len(clusters),1):.1f}x\n")

    print(f"  {'Count':>7} {'%':>6}  Template")
    print(f"  {'-'*7} {'-'*6}  {'-'*50}")
    for cluster in clusters[:top_n]:
        pct = cluster.size / total * 100
        template = cluster.get_template()
        # Truncate long templates
        if len(template) > 100:
            template = template[:97] + "..."
        print(f"  {cluster.size:>7} {pct:>5.1f}%  {template}")

    if len(clusters) > top_n:
        remaining = sum(c.size for c in clusters[top_n:])
        print(f"\n  ... and {len(clusters)-top_n} more templates ({remaining} lines)")


def analyze_rare_templates(miner: TemplateMiner, threshold: int):
    print_section(f"RARE TEMPLATES (count <= {threshold})")

    clusters = sorted(miner.drain.clusters, key=lambda c: c.size)
    rare = [c for c in clusters if c.size <= threshold]

    if not rare:
        print(f"  No templates with count <= {threshold}")
        return

    print(f"  Found {len(rare)} rare templates:\n")
    for cluster in rare:
        print(f"  [{cluster.size}x] {cluster.get_template()[:120]}")


def analyze_temporal_patterns(events: list[dict], cluster_ids: list[int]):
    """Detect templates that appear in bursts or only at specific times."""
    print_section("TEMPORAL PATTERNS")

    # Group by cluster ID and find time spans
    by_cluster: dict[int, list[datetime]] = defaultdict(list)
    for ev, cid in zip(events, cluster_ids):
        by_cluster[cid].append(ev["timestamp"])

    if not events:
        return

    total_span = (events[-1]["timestamp"] - events[0]["timestamp"]).total_seconds()
    if total_span == 0:
        print("  Log spans 0 seconds, skipping temporal analysis.")
        return

    # Find bursty patterns (all occurrences within a small fraction of total time)
    bursty = []
    for cid, timestamps in by_cluster.items():
        if len(timestamps) < 3:
            continue
        cluster_span = (max(timestamps) - min(timestamps)).total_seconds()
        concentration = cluster_span / total_span if total_span > 0 else 1
        if concentration < 0.1 and len(timestamps) >= 5:
            bursty.append((cid, len(timestamps), cluster_span, min(timestamps)))

    if bursty:
        print("  Bursty patterns (>= 5 events concentrated in < 10% of log timespan):\n")
        bursty.sort(key=lambda x: -x[1])
        for cid, count, span, first_ts in bursty[:15]:
            print(f"    cluster={cid:>4}  count={count:>4}  span={span:.1f}s  start={first_ts.strftime('%H:%M:%S')}")
    else:
        print("  No bursty patterns detected.")

    # Find periodic patterns
    print("\n  Event rate by minute:")
    minute_counts: dict[str, int] = defaultdict(int)
    for ev in events:
        minute_key = ev["timestamp"].strftime("%H:%M")
        minute_counts[minute_key] += 1

    for minute, count in sorted(minute_counts.items()):
        bar = "#" * min(count // 5, 60)
        print(f"    {minute}  {count:>5}  {bar}")


def analyze_level_by_template(events: list[dict], cluster_ids: list[int], miner: TemplateMiner):
    """Show which templates produce warnings/errors."""
    print_section("TEMPLATES BY LOG LEVEL")

    cluster_levels: dict[int, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    for ev, cid in zip(events, cluster_ids):
        cluster_levels[cid][ev["level"]] += 1

    # Only show clusters that have WARN or ERROR
    problem_clusters = {
        cid: levels
        for cid, levels in cluster_levels.items()
        if levels.get("WARN", 0) > 0 or levels.get("ERROR", 0) > 0
    }

    if not problem_clusters:
        print("  No templates with WARN or ERROR level.")
        return

    print(f"  Templates producing warnings/errors:\n")
    for cid in sorted(problem_clusters, key=lambda c: -(problem_clusters[c].get("ERROR", 0) * 1000 + problem_clusters[c].get("WARN", 0))):
        levels = problem_clusters[cid]
        cluster = next((c for c in miner.drain.clusters if c.cluster_id == cid), None)
        template = cluster.get_template()[:100] if cluster else f"cluster#{cid}"
        level_str = ", ".join(f"{l}={c}" for l, c in sorted(levels.items()))
        print(f"    [{level_str}]")
        print(f"    {template}\n")


def analyze_module_distribution(events: list[dict]):
    print_section("MODULE DISTRIBUTION")

    module_counts: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    for ev in events:
        short_module = ev["module"].split("::")[-1]
        module_counts[short_module][ev["level"]] += 1

    sorted_modules = sorted(module_counts.items(), key=lambda x: -sum(x[1].values()))
    print(f"  {'Module':<35} {'Total':>7} {'ERR':>5} {'WARN':>5} {'INFO':>6} {'DBG':>6} {'TRC':>7}")
    print(f"  {'-'*35} {'-'*7} {'-'*5} {'-'*5} {'-'*6} {'-'*6} {'-'*7}")
    for module, levels in sorted_modules[:25]:
        total = sum(levels.values())
        e = levels.get("ERROR", 0)
        w = levels.get("WARN", 0)
        i = levels.get("INFO", 0)
        d = levels.get("DEBUG", 0)
        t = levels.get("TRACE", 0)
        print(f"  {module:<35} {total:>7} {e:>5} {w:>5} {i:>6} {d:>6} {t:>7}")


def main():
    parser = argparse.ArgumentParser(description="Log template mining with Drain3")
    parser.add_argument("logfile", type=Path, help="Path to the log file")
    parser.add_argument("--top", type=int, default=20, help="Show top N templates (default: 20)")
    parser.add_argument("--anomaly-threshold", type=int, default=2,
                        help="Templates with count <= this are 'rare' (default: 2)")
    parser.add_argument("--show-rare", action="store_true", help="Show rare/anomalous templates")
    parser.add_argument("--min-level", choices=["TRACE", "DEBUG", "INFO", "WARN", "ERROR"], default="TRACE",
                        help="Minimum log level to include (default: TRACE)")
    args = parser.parse_args()

    level_order = {"TRACE": 0, "DEBUG": 1, "INFO": 2, "WARN": 3, "ERROR": 4}
    min_level = level_order[args.min_level]

    print(f"Parsing {args.logfile} ...")
    lines = args.logfile.read_text().splitlines()

    events = []
    for line in lines:
        ev = parse_line(line)
        if ev and level_order.get(ev["level"], 0) >= min_level:
            events.append(ev)

    print(f"Parsed {len(events)} events from {len(lines)} lines")

    if not events:
        print("No events to analyze.")
        sys.exit(1)

    # Run Drain3 template mining
    print("Running Drain3 template mining ...")
    miner = create_template_miner()
    cluster_ids = []
    for ev in events:
        result = miner.add_log_message(ev["message"])
        cluster_ids.append(result["cluster_id"])

    print(f"Discovered {len(miner.drain.clusters)} templates")

    # Analyses
    analyze_module_distribution(events)
    analyze_templates(miner, args.top)
    if args.show_rare:
        analyze_rare_templates(miner, args.anomaly_threshold)
    analyze_level_by_template(events, cluster_ids, miner)
    analyze_temporal_patterns(events, cluster_ids)


if __name__ == "__main__":
    main()
