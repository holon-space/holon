#!/usr/bin/env python3
"""Measure each track of scripts/gate-integrate.sh against its bound, from the runs the flow already made.

    scripts/integrate-track-walls.py [--runs N]

Reads (never writes) the flow's integrate job records with their `.trackN` records and logs, the gate-steps
ledger and the machine load samples. Prints one table row per green integrate run of the current TRACKS layout,
the runs it refused with the reason, the gate-load floor and the verdict.

A track's wall: tracks 1 and 2 from their job record (start to end, so a wait for the quiet hold or a build slot
counts); track 0 runs in the gate's own job, so from the tracks' launch to the end of its last step.
Full cargo: the job log's admission line `cargo N/L` has N plus the seats the job takes later >= L; an integrate
job's tracks 1 and 2 take two, so it is full when admitted at 4/6 or more.
Gate load: admitted at a full cargo and every track's first step started at a 1-min load >= the floor, the lower
quartile of the 1-min load 10 min after each full-cargo admission of any job in the last 14 days.
Every ended integrate job is a table row or a refusal that names it.
Verdict: over the last N qualifying runs, the 80th percentile wall of every track is <= the bound; fewer than N
qualifying runs give no verdict.
"""
import argparse
import bisect
import json
import math
import re
import subprocess
import sys
from collections import Counter
from datetime import datetime, timedelta
from pathlib import Path

# The integrate run takes as long as its longest track; the landing queue needs it under this.
BOUND_S = 1500
RUNS = 10
FLOOR_DAYS = 14
FLOOR_LAG = timedelta(minutes=10)
SCRIPT_DIR = Path(__file__).resolve().parent
HOME = Path.home()
JOBS = HOME / ".cache/holon-flow/runtime/holon-space-holon-work-2/jobs"
GATE_STEPS = HOME / ".cache/holon-build/gate-steps.jsonl"
MACHINE = HOME / ".cache/holon-build/machine.jsonl"
ADMISSION = re.compile(r"^(?:\S+Z)?\s+cargo (\d+)/(\d+):")
# Tracks 1 and 2 of an integrate job each take a cargo seat after the job's own admission.
INTEGRATE_TRACK_SEATS = 2
GATE_LINE = re.compile(r"gate-integrate\.sh: (\S+) \(pid \d+\), tip ([0-9a-f]+),")


def ts(s):
    return datetime.fromisoformat(s.replace("Z", "+00:00"))


def parse_tracks(script_text):
    """The step names of each TRACKS entry of gate-integrate.sh, in order."""
    m = re.search(r"^TRACKS=\(\n(.*?)^\)", script_text, re.M | re.S)
    if not m:
        raise ValueError("no TRACKS=( ... ) array in gate-integrate.sh")
    return [[e.split(":", 1)[1] for e in line.strip().strip('"').split()] for line in m.group(1).splitlines()]


def check_layout(tracks, names):
    """Refuses a layout that does not hold every name exactly once, naming the dropped and doubled steps."""
    held, want = Counter(s for t in tracks for s in t), Counter(names)
    dropped = sorted(set(want) - set(held))
    doubled = sorted(s for s, k in held.items() if k > 1)
    extra = sorted(set(held) - set(want))
    if dropped or doubled or extra:
        raise ValueError(f"layout: dropped {dropped}, doubled {doubled}, unknown {extra}")


def planned_over(tracks, times, bound):
    return max(sum(times[s] for s in t) for t in tracks) > bound


def nearest_rank(values, q):
    return sorted(values)[math.ceil(q * len(values)) - 1]


def measured_verdict(runs, bound, n):
    last = [r for r in runs if r["qualifies"]][-n:]
    if len(last) < n:
        return {"verdict": "no verdict", "runs": len(last)}
    p80 = [nearest_rank([r["walls"][t] for r in last], 0.8) for t in range(len(last[0]["walls"]))]
    return {"verdict": "over" if max(p80) > bound else "under", "p80": p80, "runs": n}


def read_run(gate, track_records, rows, log, tracks, layout_at):
    """One green integrate run of `tracks`, or ValueError naming the gate and why it is no sample.

    `layout_at(tip)` is the TRACKS layout of the run's tip. Every track step needs exactly one row; a step before
    the tracks may be missing, as it is no part of a wall.
    """
    gm = GATE_LINE.search(log)
    if not gm:
        raise ValueError("no gate-integrate.sh line in its log")
    gid = gm.group(1)
    head = log[:gm.start()].splitlines()
    adm = next((m for m in map(ADMISSION.match, head) if m), None)
    if layout_at(gm.group(2)) != tracks:
        raise ValueError(f"{gid}: tip {gm.group(2)} runs another TRACKS layout")
    if gate.get("exit") != 0:
        raise ValueError(f"{gid}: integrate job exit {gate.get('exit')}")
    missing = [i for i in range(1, len(tracks)) if i not in track_records]
    if missing:
        raise ValueError(f"{gid}: no job record of track(s) {missing} (unselected or older layout)")
    lo, hi = ts(gate["startedAt"]), ts(gate["endedAt"])
    mine = [r for r in rows if r["gate"] == gid and lo <= ts(r["start"]) <= hi]
    count = Counter(r["name"] for r in mine)
    absent = [s for t in tracks for s in t if count[s] == 0]
    twice = sorted(s for s, k in count.items() if k > 1)
    if absent or twice:
        raise ValueError(f"{gid}: steps without a row {absent}, with more than one {twice}")
    red = sorted(r["name"] for r in mine if r["exit"] != 0)
    if red:
        raise ValueError(f"{gid}: red step(s) {red}")
    by = {r["name"]: r for r in mine}
    for i, t in enumerate(tracks):
        for a, b in zip(t, t[1:]):
            if ts(by[b]["start"]) < ts(by[a]["end"]):
                raise ValueError(f"{gid}: track {i} ran {b} before {a} ended: another layout")
    launch = min(ts(track_records[i]["startedAt"]) for i in track_records)
    walls = [(ts(by[tracks[0][-1]]["end"]) - launch).total_seconds()]
    walls += [(ts(track_records[i]["endedAt"]) - ts(track_records[i]["startedAt"])).total_seconds()
              for i in range(1, len(tracks))]
    return {
        "gate": gid,
        "job": gate["jobId"],
        "log": gate.get("log"),
        "admitted": (int(adm.group(1)), int(adm.group(2))) if adm else None,
        "walls": walls,
        "steps": [[(s, by[s]["wall_s"]) for s in t] for t in tracks],
        "load_start": [by[t[0]]["load_start"] for t in tracks],
        "load_max": [max(by[s]["load_max"] for s in t) for t in tracks],
    }


def is_integrate(rec):
    return rec.get("kind") == "integrate" and ".track" not in rec["jobId"]


def full_cargo(n, limit, own_seats):
    """The admission count n already holds the job's first seat; `own_seats` are the seats it takes later."""
    return n + own_seats >= limit


def qualifies(run, floor):
    adm = run["admitted"]
    return adm is not None and full_cargo(*adm, INTEGRATE_TRACK_SEATS) and min(run["load_start"]) >= floor


def log_head(path, lines=40):
    p = Path(path)
    if not p.is_file():
        return None
    with p.open(errors="replace") as f:
        return "".join(f.readline() for _ in range(lines))


def gate_load_floor(records, machine_path, until):
    """(floor, sample count): the lower quartile of the 1-min load 10 min after each full-cargo admission."""
    samples = []
    with open(machine_path) as f:
        for line in f:
            s = json.loads(line)
            samples.append((ts(s["ts"]), s["load1"]))
    samples.sort()
    keys = [t for t, _ in samples]
    since = until - timedelta(days=FLOOR_DAYS)
    loads = []
    for rec in records:
        head = log_head(rec["log"]) if rec.get("log") else None
        m = next((m for m in map(ADMISSION.match, (head or "").splitlines()) if m), None)
        own = INTEGRATE_TRACK_SEATS if is_integrate(rec) else 0
        if not m or not full_cargo(int(m.group(1)), int(m.group(2)), own) or not since <= ts(rec["startedAt"]) <= until:
            continue
        k = bisect.bisect_left(keys, ts(rec["startedAt"]) + FLOOR_LAG)
        if k < len(samples):
            loads.append(samples[k][1])
    if not loads:
        raise ValueError(f"no full-cargo admission with a load sample since {since:%Y-%m-%dT%H:%MZ}")
    return nearest_rank(loads, 0.25), len(loads)


def read_runs(records, rows, tracks, layout_at):
    """(runs, refusals) of every ended integrate job: each one is a run or a refusal naming the job."""
    by_id = {r["jobId"]: r for r in records}
    runs, refusals = [], []
    for g in sorted((r for r in records if is_integrate(r) and r.get("endedAt")), key=lambda r: r["startedAt"]):
        trk = {i: by_id[f"{g['jobId']}.track{i}"] for i in range(1, len(tracks)) if f"{g['jobId']}.track{i}" in by_id}
        try:
            runs.append(read_run(g, trk, rows, log_head(g["log"], 200) or "", tracks, layout_at))
        except ValueError as e:
            refusals.append(f"{g['jobId']}: {e}")
    return runs, refusals


def layout_at(tip):
    r = subprocess.run(["jj", "--ignore-working-copy", "file", "show", "-r", tip, "scripts/gate-integrate.sh"],
                       cwd=SCRIPT_DIR.parent, capture_output=True, text=True)
    if r.returncode != 0:
        raise ValueError(f"tip {tip} is not in this repo: {r.stderr.strip()}")
    return parse_tracks(r.stdout)


def fmt_steps(steps):
    return ", ".join(f"{s} {w:.0f}" for s, w in steps)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--runs", type=int, default=RUNS)
    args = ap.parse_args()

    tracks = parse_tracks((SCRIPT_DIR / "gate-integrate.sh").read_text())
    step_names = subprocess.run([str(SCRIPT_DIR / "gate-integrate.sh"), "--steps"], check=True,
                                capture_output=True, text=True).stdout.split()
    check_layout(tracks, [s for s in step_names if any(s in t for t in tracks)])
    if len(tracks) - 1 != INTEGRATE_TRACK_SEATS:
        raise ValueError(f"{len(tracks)} tracks, but INTEGRATE_TRACK_SEATS counts {INTEGRATE_TRACK_SEATS} track seats")
    rows = [json.loads(line) for line in GATE_STEPS.open()]
    records = [json.loads(p.read_text()) for p in JOBS.glob("*.json")]
    until = max(ts(r["endedAt"]) for r in records if is_integrate(r) and r.get("endedAt"))
    floor, n_floor = gate_load_floor(records, MACHINE, until)
    runs, refusals = read_runs(records, rows, tracks, layout_at)
    for run in runs:
        run["qualifies"] = qualifies(run, floor)

    print(f"layout: {len(tracks)} tracks: " + " | ".join(" ".join(t) for t in tracks))
    print(f"gate-load floor: {floor:.1f} (p25 of the 1-min load {FLOOR_LAG} after {n_floor} cargo-full admissions,"
          f" last {FLOOR_DAYS} days)")
    print(f"bound: {BOUND_S} s per track, p80 over the last {args.runs} qualifying runs\n")
    print("| gate | job | cargo at admission | track | wall s | steps (wall_s) | load_start | load_max | gate load |")
    print("|---|---|---|---|---|---|---|---|---|")
    for r in runs:
        adm = f"{r['admitted'][0]}/{r['admitted'][1]}" if r["admitted"] else "no line"
        for i in range(len(tracks)):
            print(f"| {r['gate']} | {r['job']} | {adm} | {i} | {r['walls'][i]:.0f} | {fmt_steps(r['steps'][i])} |"
                  f" {r['load_start'][i]:.1f} | {r['load_max'][i]:.1f} | {'yes' if r['qualifies'] else 'no'} |")
    print("\nrefused:")
    for msg in refusals:
        print(f"- {msg}")
    print()
    for label, pool in (("gate load", runs), ("any load (information only)", [dict(r, qualifies=True) for r in runs])):
        print(f"verdict at {label}: {json.dumps(measured_verdict(pool, BOUND_S, args.runs))}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
