#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
Extract numeric metrics from holon tracing logs and display as ASCII sparklines.

Auto-detects:
  - RSS memory (from MemoryMonitor)
  - MCP sync cycle durations (Re-syncing → Full sync diff)
  - Record counts per sync
  - Transaction latencies (BEGIN → COMMIT)
  - Event rate per second

Usage:
    uv run scripts/analyze-log-metrics.py /tmp/holon.log
    uv run scripts/analyze-log-metrics.py /tmp/holon.log --width 60
"""

import argparse
import re
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path


TIMESTAMP_RE = re.compile(
    r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+Z)\s+"
    r"(TRACE|DEBUG|INFO|WARN|ERROR)\s+"
    r"([\w:]+):\s+"
    r"(.*)"
)

SPARKLINE_CHARS = "▁▂▃▄▅▆▇█"


def parse_timestamp(line: str) -> tuple[datetime, str] | None:
    m = TIMESTAMP_RE.match(line.strip())
    if not m:
        return None
    ts = datetime.fromisoformat(m.group(1).replace("Z", "+00:00"))
    msg = m.group(4).strip()
    return ts, msg


def sparkline(values: list[float], width: int = 50) -> str:
    if not values:
        return ""
    # Downsample if too many points
    if len(values) > width:
        chunk_size = len(values) / width
        downsampled = []
        for i in range(width):
            start = int(i * chunk_size)
            end = int((i + 1) * chunk_size)
            chunk = values[start:end]
            downsampled.append(sum(chunk) / len(chunk) if chunk else 0)
        values = downsampled

    lo, hi = min(values), max(values)
    span = hi - lo if hi > lo else 1
    return "".join(
        SPARKLINE_CHARS[min(int((v - lo) / span * (len(SPARKLINE_CHARS) - 1)), len(SPARKLINE_CHARS) - 1)]
        for v in values
    )


def format_duration(seconds: float) -> str:
    if seconds < 1:
        return f"{seconds*1000:.0f}ms"
    return f"{seconds:.1f}s"


def print_metric(name: str, values: list[float], timestamps: list[datetime], unit: str, width: int):
    if not values:
        return
    print(f"\n  {name}")
    print(f"  {'─' * (width + 20)}")

    avg = sum(values) / len(values)
    lo, hi = min(values), max(values)
    t_start = timestamps[0].strftime("%H:%M:%S")
    t_end = timestamps[-1].strftime("%H:%M:%S")

    print(f"  {sparkline(values, width)}")
    print(f"  {t_start:<{width//2}}{t_end:>{width - width//2}}")
    print(f"  n={len(values)}  min={lo:.1f}{unit}  avg={avg:.1f}{unit}  max={hi:.1f}{unit}")

    # Highlight outliers (> 2 stddev from mean)
    if len(values) > 3:
        variance = sum((v - avg) ** 2 for v in values) / len(values)
        stddev = variance ** 0.5
        threshold = avg + 2 * stddev
        outliers = [(t, v) for t, v in zip(timestamps, values) if v > threshold]
        if outliers:
            print(f"  outliers (>{threshold:.1f}{unit}):")
            for t, v in outliers[:5]:
                print(f"    {t.strftime('%H:%M:%S')}  {v:.1f}{unit}")


def extract_rss_memory(lines: list[tuple[datetime, str]]) -> tuple[list[float], list[datetime]]:
    values, timestamps = [], []
    pattern = re.compile(r"RSS ([\d.]+)MB")
    for ts, msg in lines:
        m = pattern.search(msg)
        if m:
            values.append(float(m.group(1)))
            timestamps.append(ts)
    return values, timestamps


def extract_rss_delta(lines: list[tuple[datetime, str]]) -> tuple[list[float], list[datetime]]:
    values, timestamps = [], []
    pattern = re.compile(r"delta ([+-][\d.]+)MB")
    for ts, msg in lines:
        m = pattern.search(msg)
        if m:
            values.append(float(m.group(1)))
            timestamps.append(ts)
    return values, timestamps


def extract_sync_durations(lines: list[tuple[datetime, str]]) -> dict[str, tuple[list[float], list[datetime]]]:
    by_entity: dict[str, tuple[list[float], list[datetime]]] = defaultdict(lambda: ([], []))
    pending: dict[str, datetime] = {}

    entity_re = re.compile(r"Re-syncing entity '(\w+)'")
    diff_re = re.compile(r"Full sync diff for '(\w+)'")
    fail_re = re.compile(r"Failed to resync")

    for ts, msg in lines:
        m = entity_re.search(msg)
        if m:
            pending[m.group(1)] = ts
            continue

        m = diff_re.search(msg)
        if m:
            entity = m.group(1)
            if entity in pending:
                duration = (ts - pending[entity]).total_seconds()
                by_entity[entity][0].append(duration)
                by_entity[entity][1].append(ts)
                del pending[entity]
            continue

        if fail_re.search(msg):
            pending.clear()

    return dict(by_entity)


def extract_sync_record_counts(lines: list[tuple[datetime, str]]) -> dict[str, tuple[list[float], list[datetime]]]:
    by_entity: dict[str, tuple[list[float], list[datetime]]] = defaultdict(lambda: ([], []))
    pattern = re.compile(r"Got (\d+) records.*?'(\w+)'")

    # Match "Got N records for entity 'X'" pattern
    for ts, msg in lines:
        m = re.search(r"Got (\d+) records for entity '(\w+)'", msg)
        if m:
            count = float(m.group(1))
            entity = m.group(2)
            by_entity[entity][0].append(count)
            by_entity[entity][1].append(ts)

    return dict(by_entity)


def extract_transaction_latencies(lines: list[tuple[datetime, str]]) -> tuple[list[float], list[datetime]]:
    values, timestamps = [], []
    pending_tx = None

    for ts, msg in lines:
        if "actor_tx_begin" in msg:
            pending_tx = ts
        elif "actor_tx_commit" in msg and pending_tx:
            delta = (ts - pending_tx).total_seconds()
            values.append(delta * 1000)  # ms
            timestamps.append(ts)
            pending_tx = None

    return values, timestamps


def extract_event_rate(lines: list[tuple[datetime, str]], bucket_seconds: float = 1.0) -> tuple[list[float], list[datetime]]:
    if not lines:
        return [], []

    start = lines[0][0]
    buckets: dict[int, int] = defaultdict(int)

    for ts, _ in lines:
        bucket = int((ts - start).total_seconds() / bucket_seconds)
        buckets[bucket] += 1

    if not buckets:
        return [], []

    max_bucket = max(buckets.keys())
    values = [float(buckets.get(i, 0)) for i in range(max_bucket + 1)]
    timestamps = [
        start.replace(microsecond=0)  # approximate
        for i in range(max_bucket + 1)
    ]
    return values, timestamps


def extract_batch_sizes(lines: list[tuple[datetime, str]]) -> tuple[list[float], list[datetime]]:
    values, timestamps = [], []
    pattern = re.compile(r"Applying batch of (\d+) changes")
    for ts, msg in lines:
        m = pattern.search(msg)
        if m:
            values.append(float(m.group(1)))
            timestamps.append(ts)
    return values, timestamps


def main():
    parser = argparse.ArgumentParser(description="Extract metrics from holon logs as sparklines")
    parser.add_argument("logfile", type=Path, help="Path to the log file")
    parser.add_argument("--width", type=int, default=50, help="Sparkline width in characters (default: 50)")
    args = parser.parse_args()

    print(f"Parsing {args.logfile} ...")
    raw_lines = args.logfile.read_text().splitlines()
    lines = []
    for line in raw_lines:
        parsed = parse_timestamp(line)
        if parsed:
            lines.append(parsed)

    print(f"Parsed {len(lines)} events from {len(raw_lines)} lines")
    if not lines:
        print("No events found.")
        sys.exit(1)

    time_range = (lines[-1][0] - lines[0][0]).total_seconds()
    print(f"Time range: {lines[0][0].strftime('%H:%M:%S')} → {lines[-1][0].strftime('%H:%M:%S')} ({time_range:.0f}s)")

    w = args.width

    # Event rate
    print("\n" + "=" * 70)
    print("  EVENT RATE")
    print("=" * 70)
    values, timestamps = extract_event_rate(lines, bucket_seconds=5.0)
    print_metric("Events per 5s bucket", values, timestamps, "", w)

    # Memory
    print("\n" + "=" * 70)
    print("  MEMORY")
    print("=" * 70)
    values, timestamps = extract_rss_memory(lines)
    print_metric("RSS (MB)", values, timestamps, "MB", w)
    values, timestamps = extract_rss_delta(lines)
    print_metric("RSS delta (MB)", values, timestamps, "MB", w)

    # Sync durations
    print("\n" + "=" * 70)
    print("  SYNC CYCLE DURATIONS")
    print("=" * 70)
    sync_durations = extract_sync_durations(lines)
    for entity, (values, timestamps) in sorted(sync_durations.items()):
        print_metric(f"Sync '{entity}' duration", values, timestamps, "s", w)

    # Record counts
    print("\n" + "=" * 70)
    print("  SYNC RECORD COUNTS")
    print("=" * 70)
    record_counts = extract_sync_record_counts(lines)
    for entity, (values, timestamps) in sorted(record_counts.items()):
        print_metric(f"Records '{entity}'", values, timestamps, "", w)

    # Transaction latencies
    print("\n" + "=" * 70)
    print("  TRANSACTION LATENCIES")
    print("=" * 70)
    values, timestamps = extract_transaction_latencies(lines)
    print_metric("Transaction latency", values, timestamps, "ms", w)

    # Batch sizes
    print("\n" + "=" * 70)
    print("  CACHE BATCH SIZES")
    print("=" * 70)
    values, timestamps = extract_batch_sizes(lines)
    print_metric("QueryableCache batch size", values, timestamps, "", w)


if __name__ == "__main__":
    main()
