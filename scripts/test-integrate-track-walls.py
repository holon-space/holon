#!/usr/bin/env python3
"""Properties of scripts/integrate-track-walls.py: the layout check, the planned and the measured verdict, and the
reading of one integrate run from its records.

Run: `python3 scripts/test-integrate-track-walls.py` (CASES=<n>, SEED=<s>). The fixed examples run first.
"""
import atexit
import importlib.util
import os
import json
import random
import shutil
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "integrate-track-walls.py"
PRE = ["fmt", "deny", "machete", "gate-compile"]
NAMES = [f"step{i}" for i in range(17)]
T0 = datetime(2026, 1, 1, tzinfo=timezone.utc)
# One directory for the whole run: a directory per case made file creation and removal the bulk of the run time.
SCRATCH = Path(tempfile.mkdtemp(prefix="test-integrate-track-walls-"))
atexit.register(shutil.rmtree, SCRATCH)

# Track step sums (s) of 14 integrate runs of the TRACKS layout, oldest first: track 0 is over at the p80 of the last 10.
FOURTEEN = [(809, 915, 523), (1192, 1130, 999), (1028, 963, 388), (1112, 950, 797), (1557, 919, 1705),
            (1980, 1401, 333), (1287, 1333, 385), (1006, 1920, 292), (1415, 1321, 375), (2147, 1911, 1006),
            (1226, 1280, 709), (1388, 1310, 498), (549, 961, 258), (454, 597, 141)]


def load():
    spec = importlib.util.spec_from_file_location("integrate_track_walls", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def layout(rng, names=NAMES):
    shuffled = list(names)
    rng.shuffle(shuffled)
    cuts = sorted(rng.sample(range(1, len(shuffled)), 2))
    return [shuffled[:cuts[0]], shuffled[cuts[0]:cuts[1]], shuffled[cuts[1]:]]


def refused(fn, *args):
    try:
        fn(*args)
    except ValueError as e:
        return str(e)
    return None


def layout_with_every_step_once_is_accepted_and_a_dropped_or_doubled_one_is_named(m, rng):
    tracks = layout(rng)
    m.check_layout(tracks, NAMES)
    victim = rng.choice(NAMES)
    dropped = [[s for s in t if s != victim] for t in tracks]
    msg = refused(m.check_layout, dropped, NAMES)
    assert msg and victim in msg, f"dropping {victim} must be refused naming it: {msg!r}"
    doubled = [list(t) for t in tracks]
    doubled[rng.randrange(3)].append(victim)
    msg = refused(m.check_layout, doubled, NAMES)
    assert msg and victim in msg, f"doubling {victim} must be refused naming it: {msg!r}"


def planned_verdict_only_grows_with_step_time(m, rng):
    tracks = layout(rng)
    times = {s: rng.uniform(0, 1200) for s in NAMES}
    before = m.planned_over(tracks, times, m.BOUND_S)
    assert before == (max(sum(times[s] for s in t) for t in tracks) > m.BOUND_S)
    grown = dict(times)
    grown[rng.choice(NAMES)] += rng.uniform(0, 600)
    assert not before or m.planned_over(tracks, grown, m.BOUND_S), "more step time turned an over plan into under"
    for t in tracks:
        rng.shuffle(t)
    assert m.planned_over(tracks, times, m.BOUND_S) == before, "the step order inside a track changed the plan"


def draw_runs(rng, k):
    return [m_run(rng.random() < 0.7, [rng.uniform(100, 2500) for _ in range(3)]) for _ in range(k)]


def m_run(qualifies, walls):
    return {"qualifies": qualifies, "walls": walls}


def measured_verdict_obeys_the_p80_rule(m, rng):
    runs = draw_runs(rng, rng.randint(0, 20))
    v = m.measured_verdict(runs, m.BOUND_S, m.RUNS)
    last = [r for r in runs if r["qualifies"]][-m.RUNS:]
    if len(last) < m.RUNS:
        assert v["verdict"] == "no verdict", f"{len(last)} qualifying runs gave {v}"
        return
    p80 = [m.nearest_rank([r["walls"][t] for r in last], 0.8) for t in range(3)]
    assert v["p80"] == p80, (v, p80)
    assert v["verdict"] == ("over" if max(p80) > m.BOUND_S else "under"), v
    padded = runs + [m_run(False, [0.0, 0.0, 0.0])]
    assert m.measured_verdict(padded, m.BOUND_S, m.RUNS) == v, "a run that does not qualify changed the verdict"
    grown = [dict(r, walls=list(r["walls"])) for r in runs]
    grown[rng.randrange(len(grown))]["walls"][rng.randrange(3)] += rng.uniform(0, 3000)
    if v["verdict"] == "over":
        assert m.measured_verdict(grown, m.BOUND_S, m.RUNS)["verdict"] == "over", "a longer track turned over into under"


def iso(t):
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


def draw_gate(rng):
    """(tracks, records, rows, step sums per track) of one green integrate run with every step present."""
    tracks = layout(rng)
    t = T0 + timedelta(seconds=rng.randint(0, 10**6))
    started = t
    rows = []
    for name in PRE:
        wall = rng.randint(1, 60)
        rows.append({"gate": "gateX", "name": name, "start": iso(t), "end": iso(t + timedelta(seconds=wall)),
                     "wall_s": float(wall), "exit": 0, "load_start": 50.0, "load_max": 60.0})
        t += timedelta(seconds=wall)
    launch = t
    ends, sums = [], []
    for track in tracks:
        s = launch + timedelta(seconds=rng.randint(0, 300))
        total = 0
        for name in track:
            s += timedelta(seconds=rng.randint(0, 3))
            wall = rng.randint(1, 900)
            rows.append({"gate": "gateX", "name": name, "start": iso(s), "end": iso(s + timedelta(seconds=wall)),
                         "wall_s": float(wall), "exit": 0, "load_start": rng.uniform(10, 300),
                         "load_max": rng.uniform(300, 400)})
            s += timedelta(seconds=wall)
            total += wall
        ends.append(s + timedelta(seconds=rng.randint(0, 2)))
        sums.append(total)
    rng.shuffle(rows)
    n = rng.randint(1, 6)
    stamp = rng.choice(["", "2026-01-01T00:00:00Z "])
    log = f"{stamp}  cargo {n}/6: #1\n{stamp}gate-integrate.sh: gateX (pid 1), tip abc, cards: 1\n"
    gate = {"jobId": "wX.integrate", "startedAt": iso(started), "endedAt": iso(max(ends) + timedelta(seconds=5)), "exit": 0}
    track_records = {i: {"jobId": f"wX.integrate.track{i}", "startedAt": iso(launch), "endedAt": iso(ends[i]), "exit": 0}
                     for i in (1, 2)}
    return tracks, gate, track_records, rows, log, sums, n


def a_read_run_has_track_walls_no_shorter_than_its_step_sums(m, rng):
    tracks, gate, track_records, rows, log, sums, n = draw_gate(rng)
    run = m.read_run(gate, track_records, rows, log, tracks, lambda tip: tracks)
    assert run["gate"] == "gateX" and run["admitted"] == (n, 6), run
    for i in range(3):
        assert run["walls"][i] >= sums[i], f"track {i} wall {run['walls'][i]} below its step sum {sums[i]}"
        assert run["steps"][i] == [(s, next(r["wall_s"] for r in rows if r["name"] == s)) for s in tracks[i]]
        want_max = max(r["load_max"] for r in rows if r["name"] in tracks[i])
        assert run["load_max"][i] == want_max, f"track {i} load_max {run['load_max'][i]} is not its steps' max {want_max}"


def a_read_run_takes_each_track_wall_from_its_records(m, rng):
    tracks, gate, track_records, rows, log, _, _ = draw_gate(rng)
    run = m.read_run(gate, track_records, rows, log, tracks, lambda tip: tracks)
    at = lambda s: datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    launch = min(at(r["startedAt"]) for r in track_records.values())
    last0 = next(r for r in rows if r["name"] == tracks[0][-1])
    want = [(at(last0["end"]) - launch).total_seconds()]
    want += [(at(track_records[i]["endedAt"]) - at(track_records[i]["startedAt"])).total_seconds() for i in (1, 2)]
    assert run["walls"] == want, f"walls {run['walls']} are not the record spans {want}"
    first = [next(r["load_start"] for r in rows if r["name"] == t[0]) for t in tracks]
    assert run["load_start"] == first, f"load_start {run['load_start']} is not each track's first step {first}"


def scratch_file(text):
    path = SCRATCH / f"{abs(hash(text))}.log"
    if not path.exists():
        path.write_text(text)
    return path


def admission_log(n, cap, stamped):
    return scratch_file(f"{'2026-01-01T00:00:00Z ' if stamped else ''}  cargo {n}/{cap}: #1\n")


def every_ended_integrate_job_is_a_run_or_a_refusal_naming_it(m, rng):
    tracks, gate, track_records, rows, log, _, _ = draw_gate(rng)
    dropped = rng.choice([[], [1], [2], [1, 2]])
    records = [dict(gate, kind="integrate", log=str(scratch_file(log)))]
    records += [dict(r, kind="integrate") for i, r in track_records.items() if i not in dropped]
    runs, refusals = m.read_runs(records, rows, tracks, lambda tip: tracks)
    assert len(runs) == (0 if dropped else 1) and len(refusals) == (1 if dropped else 0), (dropped, runs, refusals)
    assert all(gate["jobId"] in msg for msg in refusals), refusals


def a_partial_or_red_run_is_refused_naming_the_gate(m, rng):
    tracks, gate, track_records, rows, log, _, _ = draw_gate(rng)
    at_tip = tracks
    broken = rng.choice(["drop row", "red row", "red gate", "missing track record", "double row", "other layout",
                         "other layout at tip"])
    if broken == "drop row":
        victim = rng.choice([r for r in rows if r["name"] not in PRE])
        rows = [r for r in rows if r is not victim]
    elif broken == "red row":
        rng.choice(rows)["exit"] = rng.randint(1, 13)
    elif broken == "red gate":
        gate = dict(gate, exit=rng.randint(1, 13))
    elif broken == "missing track record":
        del track_records[rng.choice([1, 2])]
    elif broken == "double row":
        rows = rows + [dict(rng.choice([r for r in rows if r["name"] not in PRE]))]
    elif broken == "other layout at tip":
        at_tip = layout(rng)
        if at_tip == tracks:
            return
    else:
        multi = [t for t in tracks if len(t) > 1]
        if not multi:
            return
        t = rng.choice(multi)
        t[0], t[-1] = t[-1], t[0]
    msg = refused(m.read_run, gate, track_records, rows, log, tracks, lambda tip: at_tip)
    assert msg and "gateX" in msg, f"{broken}: must be refused naming the gate, got {msg!r}"


def draw_admissions(rng):
    """Machine samples every 30-120 min and job records whose logs carry a cargo admission line, some outside the window.

    `full` is the ruled meter: an integrate job's two track seats count, and an admission over the limit is full.
    """
    until = T0 + timedelta(days=20)
    every = rng.randint(30, 120)
    samples = [(until - timedelta(minutes=k), round(rng.uniform(0, 400), 1)) for k in range(0, 20 * 24 * 60, every)]
    machine = SCRATCH / "machine.jsonl"
    machine.write_text("".join(json.dumps({"ts": iso(t), "load1": v}) + "\n" for t, v in samples))
    records = []
    for k in range(rng.randint(1, 10)):
        start = until - timedelta(minutes=rng.randint(30, 20 * 24 * 60 - 30))
        n, cap = rng.choice([(6, 6), (6, 6), (5, 6), (3, 6), (7, 6)])
        log = admission_log(n, cap, rng.random() < 0.5)
        kind = rng.choice(["build", "integrate"])
        records.append({"jobId": f"job{k}", "kind": kind, "startedAt": iso(start), "log": str(log),
                        "full": n + (2 if kind == "integrate" else 0) >= cap})
    records.append({"jobId": "full", "startedAt": iso(until - timedelta(hours=1)), "log": str(admission_log(6, 6, False)),
                    "full": True})
    return machine, records, until, sorted(samples)


def qualification_and_floor_follow_the_gate_load_definition(m, rng):
    machine, records, until, samples = draw_admissions(rng)
    since = until - timedelta(days=m.FLOOR_DAYS)
    at = lambda s: datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    due = [at(r["startedAt"]) + m.FLOOR_LAG for r in records if r["full"] and since <= at(r["startedAt"]) <= until]
    loads = [next(v for t, v in samples if t >= d) for d in due]
    floor, n = m.gate_load_floor(records, machine, until)
    assert (floor, n) == (m.nearest_rank(loads, 0.25), len(loads)), ((floor, n), loads)
    admitted = rng.choice([None, (6, 6), (5, 6), (rng.randint(1, 6), 6)])
    run = {"admitted": admitted, "load_start": [rng.uniform(0, 200) for _ in range(3)]}
    want = admitted is not None and admitted[0] + 2 >= admitted[1] and min(run["load_start"]) >= floor
    assert m.qualifies(run, floor) == want, (run, floor)


def draw_integrate_admission(rng):
    """An integrate run's admission line N/6 (N counts the run's own first seat) and its tracks' first-step loads.

    N = 3 is not drawn: whether 3/6 plus the run's track seats is full is open with main.
    """
    n = rng.choice([1, 2, 4, 5, 6])
    return {"admitted": (n, 6), "load_start": [rng.uniform(0, 200) for _ in range(3)]}


def an_integrate_run_counts_its_own_track_seats_toward_a_full_cargo(m, rng):
    run = draw_integrate_admission(rng)
    floor = rng.uniform(0, 200)
    want = run["admitted"][0] >= 4 and min(run["load_start"]) >= floor
    assert m.qualifies(run, floor) == want, f"admitted {run['admitted']}, floor {floor:.1f}: qualifies must be {want}"


def draw_track_count(rng):
    """A TRACKS array of 1 to 5 tracks over the step names, and a gate script that lists them for --steps."""
    k = rng.randint(1, 5)
    names = NAMES[:max(k, rng.randint(k, len(NAMES)))]
    cuts = sorted(rng.sample(range(1, len(names)), k - 1))
    tracks = [names[a:b] for a, b in zip([0] + cuts, cuts + [len(names)])]
    body = "".join('  "' + " ".join(f"{j}:{s}" for j, s in enumerate(t)) + '"\n' for t in tracks)
    gate = SCRATCH / f"tracks{k}-{len(names)}" / "gate-integrate.sh"
    gate.parent.mkdir(exist_ok=True)
    gate.write_text(f"#!/bin/sh\n# TRACKS\necho {' '.join(names)}\nexit 0\nTRACKS=(\n{body})\n")
    gate.chmod(0o755)
    return tracks, gate


def the_meter_refuses_a_layout_whose_track_count_is_not_its_track_seats(m, rng):
    tracks, gate = draw_track_count(rng)
    empty = SCRATCH / "empty.jsonl"
    empty.write_text("")
    saved = (m.SCRIPT_DIR, m.GATE_STEPS, m.JOBS, m.MACHINE, os.sys.argv)
    m.SCRIPT_DIR, m.GATE_STEPS, m.JOBS, m.MACHINE, os.sys.argv = gate.parent, empty, SCRATCH / "no-jobs", empty, ["x"]
    try:
        msg = refused(m.main)
    finally:
        m.SCRIPT_DIR, m.GATE_STEPS, m.JOBS, m.MACHINE, os.sys.argv = saved
    seats = len(tracks) - 1 != m.INTEGRATE_TRACK_SEATS
    assert seats == bool(msg and "track seats" in msg), f"{len(tracks)} tracks: refusal {msg!r}"


PROPERTIES = [
    layout_with_every_step_once_is_accepted_and_a_dropped_or_doubled_one_is_named,
    planned_verdict_only_grows_with_step_time,
    measured_verdict_obeys_the_p80_rule,
    a_read_run_has_track_walls_no_shorter_than_its_step_sums,
    a_read_run_takes_each_track_wall_from_its_records,
    a_partial_or_red_run_is_refused_naming_the_gate,
    every_ended_integrate_job_is_a_run_or_a_refusal_naming_it,
    qualification_and_floor_follow_the_gate_load_definition,
    an_integrate_run_counts_its_own_track_seats_toward_a_full_cargo,
    the_meter_refuses_a_layout_whose_track_count_is_not_its_track_seats,
]


def examples(m):
    repo_tracks = m.parse_tracks((SCRIPT.parent / "gate-integrate.sh").read_text())
    names = [s for t in repo_tracks for s in t]
    m.check_layout(repo_tracks, names)
    assert len(repo_tracks) == 3 and len(names) == 17, repo_tracks
    runs = [m_run(True, list(w)) for w in FOURTEEN]
    v = m.measured_verdict(runs, m.BOUND_S, m.RUNS)
    assert v == {"verdict": "over", "p80": [1557, 1401, 709], "runs": 10}, v
    assert m.measured_verdict(runs[:9], m.BOUND_S, m.RUNS)["verdict"] == "no verdict"
    assert m.nearest_rank([81, 110, 86, 137], 0.25) == 81
    for n, want in ((3, False), (4, True), (7, True)):
        run = {"admitted": (n, 6), "load_start": [60.0, 50.7, 90.0]}
        assert m.qualifies(run, 50.7) == want, f"an integrate run admitted at {n}/6 at the floor: qualifies {want}"
        assert not m.qualifies(run, 50.8), f"admitted {n}/6 with a first step under the floor qualified"


def main():
    m = load()
    seed = int(os.environ.get("SEED", random.randrange(2**31)))
    cases = int(os.environ.get("CASES", "300"))
    examples(m)
    rng = random.Random(seed)
    for prop in PROPERTIES:
        for _ in range(cases):
            prop(m, rng)
    print(f"ok: examples and {len(PROPERTIES)} properties x {cases} cases (SEED={seed})")


if __name__ == "__main__":
    main()
