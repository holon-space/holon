#!/usr/bin/env python3
"""Time the tests of the load known-red rows at gate load and judge each row: delete it or keep it.

    scripts/known-red-gate-load.py rows                  the rows, their rule and their tests
    scripts/known-red-gate-load.py band                  the gate-load floor (median) and p90 of now
    scripts/known-red-gate-load.py run KEY...            inside a `flow job run --queue` job: time each test of each KEY once
    scripts/known-red-gate-load.py verdict [KEY...]      the verdict of each row, from its run records only
    scripts/known-red-gate-load.py cause KEY...          inside a `flow job run --queue` job: one cause run of each test

`run` records the job's admission line `cargo N/L` of $FLOW_JOB_LOG, checks with `nextest list` that each filter
selects exactly one test (which also builds it), and reads the 1-min load right before the test. Below the floor it
prints a `skip` line and runs nothing; else it runs the test with no retries and appends one line per test to
docs/Testing/known-red-evidence/<key>/gate-load-runs.tsv of every row that names the test, with the test's log as
zstd beside it.
load_max is the highest 1-min load of the machine sampler between the test's start and end, and of load_start.
A run qualifies when load_start reaches the gate-load floor: the median of the 1-min load samples taken while an
integrate or gate job ran, over the last GATE_DAYS days.

Verdict per test, from its qualifying runs: fewer than MIN_RUNS gives no verdict.
- A bound row (seconds): delete iff every run passed and the slowest took at most half the bound.
- A rate row (base red rate): delete iff no run was red and the runs reach the rule-of-three count, 3/n <= rate/2;
  a rate that needs more than RATE_CAP runs can only be kept.
- Keep otherwise, but only once a run's load_max reached the p90 of the same samples; before that there is no verdict.
A row is deleted when every test of it is; kept when any test of it is kept.
"""
import argparse
import csv
import importlib.util
import json
import math
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EVIDENCE = ROOT / "docs/Testing/known-red-evidence"
LOGS = ROOT / "target/known-red-gate-load"
CANON_FEATURES = "holon-integration-tests/pbt,holon-integration-tests/web-arm,holon-gpui/pbt"
MIN_RUNS = 5
RATE_CAP = 40
GATE_DAYS = 7
GATE_KINDS = ("integrate", "gate")
FIELDS = ["test", "job", "admission", "started", "ended", "load_start", "load_max", "floor", "p90", "status",
          "seconds", "qualifies", "log", "red_rows"]
KNOWN_RED = re.compile(r"known-red \[([^\]]+)\]")
STATUS = re.compile(r"^\s*(PASS|FAIL|TIMEOUT|SIGSEGV|SIGABRT|SIGKILL|ABORT|LEAK-FAIL) \[\s*([0-9.]+)s\]")


class NoVerdict(Exception):
    pass


@dataclass(frozen=True)
class Test:
    binary: str
    name: str
    profile: str = "default"
    env: tuple = ()
    ignored: bool = False

    @property
    def id(self):
        case = "".join(f" {k}={v}" for k, v in self.env)
        return f"{self.binary} {self.name}{case}"


@dataclass(frozen=True)
class Row:
    rule: tuple
    tests: tuple


def gpui(name):
    # the windowed gate's own timing mode (justfile `windowed-gate`), where these rows went red
    return Test("holon-gpui::main", name, env=(("HOLON_PBT_TIMING", "measure"),))


WIDE = "holon-integration-tests"
INTEG = "holon-integration-tests::integration"
TOLERANT = "load-tolerant"
PER_CASE_BOOT = gpui("gpui_composed_windowed_loop::benchmark_windowed_per_case_boot_cost")
COMPOSED_WINDOWED = gpui("gpui_composed_windowed_loop::general_e2e_composed_pbt_windowed")
# Each bound is the one the row's red names; each rate is the red share of the row's Evidence cell.
ROWS = {
    "wide-e2e-sqlonly-typing-share-load-timeout": Row(("bound", 600.0), (Test(
        WIDE, "pbt::composed::wide_e2e::tests::sqlonly_typing_share_meets_its_floor_beside_the_loro_control",
        TOLERANT),)),
    "logseq-parity-corpus-replay-load-timeout": Row(("bound", 600.0), (Test(
        INTEG, "catalog_suite::logseq_parity_replay::logseq_parity_corpus_replay", TOLERANT),)),
    "dense-patch-cross-document-write-back-late-load": Row(("bound", 30.0), (Test(
        INTEG, "dense_patch_engine_exact::a_generated_cross_document_edit_reads_back_exactly_or_is_refused"),)),
    "refused-profile-fix-clears-refusal-load-timeout": Row(("bound", 30.0), (Test(
        "holon-app::main", "refused_profile_boots_degraded::a_refused_profile_written_after_boot_is_disclosed_until_fixed"),)),
    # The ratio prints only when it breaks 8x, so a pass carries no ratio: judged by its red rate.
    "org-deep-text-ratio-under-load": Row(("rate", 1 / 2), tuple(
        Test("holon-org-format::deep_authored_text_never_aborts", name, ignored=True)
        for name in ("a_marker_line_costs_time_linear_in_its_length",
                     "a_marked_line_renders_in_time_linear_in_its_length"))),
    # The windowed rows: a pump-cycle count or a paint wait that no pass prints, so judged by their red rate.
    "windowed-focus-not-in-bounds-under-load": Row(("rate", 1 / 2), (
        PER_CASE_BOOT, COMPOSED_WINDOWED,
        gpui("breadcrumb_resolves_from_view_root_windowed::the_breadcrumb_follows_the_view_root_and_then_the_focus"))),
    "windowed-entity-not-in-bounds-under-load": Row(("rate", 1 / 2), (PER_CASE_BOOT, COMPOSED_WINDOWED)),
    "windowed-keyword-surface-stale-done-text": Row(("rate", 1 / 2), (
        gpui("gpui_composed_windowed_loop::focused_task_block_shows_its_keyword_surface"),)),
    "windowed-previous-run-toast-box-under-load": Row(("rate", 1 / 2), (
        gpui("panic_records_seen_after_a_drawn_frame_windowed::a_crash_loop_of_long_messages_fits_a_900px_window"),)),
    "windowed-block-focus-outline-zero-rows-under-load": Row(("rate", 7 / 51), (
        gpui("block_focus_keeps_outline_windowed::a_short_window_still_paints_the_outline"),)),
    "windowed-nested-page-no-chevron-under-load": Row(("rate", 8 / 51), (
        gpui("nested_page_real_engine::a_real_engine_nested_page_paints_its_children_when_opened"),)),
    "windowed-banner-toast-paint-load-flake": Row(("rate", 2 / 5), (gpui(
        "banner_conditions_draw_as_toasts_windowed::"
        "banner_conditions_without_a_banner_surface_are_painted_as_one_toast_each"),)),
}


def verdict(rule, runs, p90):
    """'delete' or 'keep' for one test from its runs (dicts: qualifies, passed, seconds, load_max); raises NoVerdict."""
    kind, value = rule
    q = [r for r in runs if r["qualifies"]]
    if len(q) < MIN_RUNS:
        raise NoVerdict(f"{len(q)} qualifying runs, {MIN_RUNS} needed")
    clean = all(r["passed"] for r in q)
    if kind == "bound":
        if clean and max(r["seconds"] for r in q) <= value / 2:
            return "delete"
    elif kind == "rate":
        need = max(MIN_RUNS, math.ceil(6 / value))
        if clean and need <= RATE_CAP:
            if len(q) < need:
                raise NoVerdict(f"{len(q)} clean qualifying runs, {need} needed at a base rate of {value:.3f}")
            return "delete"
    else:
        raise ValueError(f"unknown rule {rule}")
    if max(r["load_max"] for r in q) < p90:
        raise NoVerdict(f"kept, but no qualifying run reached the p90 load {p90:.1f}")
    return "keep"


def row_verdict(rule, runs_by_test, p90):
    results, missing = [], []
    for test, runs in runs_by_test.items():
        try:
            results.append(verdict(rule, runs, p90))
        except NoVerdict as e:
            missing.append(f"{test}: {e}")
    if "keep" in results:
        return "keep"
    if missing:
        raise NoVerdict("; ".join(missing))
    return "delete"


def walls():
    spec = importlib.util.spec_from_file_location("integrate_track_walls", ROOT / "scripts/integrate-track-walls.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def gate_band(spans, samples):
    """(median, p90, sample count) of the (time, load) samples that fall inside any (start, end) span."""
    loads = [load for t, load in samples if any(a <= t <= b for a, b in spans)]
    if not loads:
        raise ValueError(f"no load sample inside {len(spans)} integrate/gate job spans")
    return quantile(loads, 0.5), quantile(loads, 0.9), len(loads)


def quantile(values, q):
    """Nearest rank, as the meter of scripts/integrate-track-walls.py."""
    s = sorted(values)
    return s[max(0, math.ceil(q * len(s)) - 1)]


def gate_loads(w, now):
    """gate_band over the integrate and gate jobs that ended in the last GATE_DAYS days."""
    since = now - timedelta(days=GATE_DAYS)
    spans = []
    for p in w.JOBS.glob("*.json"):
        r = json.loads(p.read_text())
        if r.get("kind") in GATE_KINDS and r.get("endedAt") and w.ts(r["endedAt"]) >= since:
            spans.append((max(w.ts(r["startedAt"]), since), w.ts(r["endedAt"])))
    with open(w.MACHINE) as f:
        samples = [(w.ts(s["ts"]), s["load1"]) for s in map(json.loads, f)]
    return gate_band(spans, [(t, load) for t, load in samples if t >= since])


def admission(w, job_log):
    head = w.log_head(job_log) or ""
    m = next((m for m in map(w.ADMISSION.match, head.splitlines()) if m), None)
    if m is None:
        raise SystemExit(f"known-red-gate-load: no `cargo N/L` admission line in {job_log}")
    return int(m.group(1)), int(m.group(2))


def load1():
    return float(subprocess.run(["sysctl", "-n", "vm.loadavg"], check=True, capture_output=True,
                                text=True).stdout.split()[1])


def load_max(w, start, end, load_start):
    with open(w.MACHINE) as f:
        loads = [s["load1"] for s in map(json.loads, f) if start <= w.ts(s["ts"]) <= end]
    return max([load_start, *loads])


def nextest(test, *args):
    filt = f"binary_id({test.binary}) & test(={test.name})"
    return ["cargo", "nextest", "run" if args else "list", "--workspace", "--features", CANON_FEATURES,
            "--ignore-default-filter", "--profile", test.profile, "-E", filt,
            *(["--run-ignored", "only"] if test.ignored else []), *args]


def run_once(w, key, test, adm, floor, p90):
    env = dict(os.environ, **dict(test.env))
    listed = subprocess.run([*nextest(test), "--message-format", "oneline"], cwd=ROOT, env=env, check=True,
                            capture_output=True, text=True).stdout.split("\n")
    if len([line for line in listed if line.strip()]) != 1:
        raise SystemExit(f"known-red-gate-load: {key}: the filter of {test.id} lists {listed}, not one test")
    load_start, started = load1(), datetime.now(timezone.utc)
    if load_start < floor:
        print(f"skip\t{test.id}\t{started:%Y-%m-%dT%H:%M:%SZ}\tload_start {load_start:.2f} < floor {floor:.2f}",
              flush=True)
        return None
    LOGS.mkdir(parents=True, exist_ok=True)
    log = LOGS / f"{key}.{os.environ['FLOW_JOB_ID']}.{started:%Y%m%dT%H%M%SZ}.log"
    with log.open("w") as out:
        subprocess.run(nextest(test, "--no-fail-fast", "--retries", "0"), cwd=ROOT, env=env, stdout=out,
                       stderr=subprocess.STDOUT)
    ended = datetime.now(timezone.utc)
    lines = log.read_text(errors="replace").splitlines()
    # nextest repeats each failing status line after its `Summary` line
    body = lines[:next((i for i, line in enumerate(lines) if line.lstrip().startswith("Summary [")), len(lines))]
    statuses = [m for m in map(STATUS.match, body) if m]
    if len(statuses) != 1:
        raise SystemExit(f"known-red-gate-load: {log} holds {len(statuses)} test status lines, not one")
    status, seconds = statuses[0].group(1), float(statuses[0].group(2))
    lmax = load_max(w, started, ended, load_start)
    red_rows = "" if status == "PASS" else classify(log)
    return {"test": test.id, "job": os.environ["FLOW_JOB_ID"], "admission": f"{adm[0]}/{adm[1]}",
            "started": f"{started:%Y-%m-%dT%H:%M:%SZ}", "ended": f"{ended:%Y-%m-%dT%H:%M:%SZ}",
            "load_start": f"{load_start:.2f}", "load_max": f"{lmax:.2f}", "floor": f"{floor:.2f}", "p90": f"{p90:.2f}",
            "status": status, "seconds": f"{seconds:.3f}",
            "qualifies": "yes" if load_start >= floor else "no",
            "log": str(log.relative_to(ROOT)), "red_rows": red_rows}


def classify(log):
    """The known-red rows that the registry's classifier names for a red log, or `novel`."""
    out = subprocess.run([ROOT / "scripts/keystone-known-reds.sh", log], cwd=ROOT, capture_output=True, text=True)
    if out.returncode == 1:
        return "novel"
    if out.returncode != 0:
        raise SystemExit(f"known-red-gate-load: classifier exit {out.returncode} on {log}: {out.stderr.strip()}")
    rows = sorted(set(KNOWN_RED.findall(out.stdout)))
    if not rows:
        raise SystemExit(f"known-red-gate-load: classifier passed {log} but named no known-red row")
    return ",".join(rows)


def passed_for(key, status, red_rows, whole_run):
    """A run passes for row `key` when it passed, or failed only on other rows' known reds (no timeout).
    A `whole_run` bound times the full test, so an earlier abort leaves it unmeasured: only a PASS counts."""
    if whole_run:
        return status == "PASS"
    return status == "PASS" or (status == "FAIL" and red_rows not in ("", "novel") and key not in red_rows.split(","))


def tsv(key):
    return EVIDENCE / key / "gate-load-runs.tsv"


def append(key, rec):
    path = tsv(key)
    path.parent.mkdir(parents=True, exist_ok=True)
    kept = path.parent / f"{Path(rec['log']).name}.zst"
    subprocess.run(["zstd", "-q", "-19", ROOT / rec["log"], "-o", kept], check=True)
    rec = dict(rec, log=str(kept.relative_to(ROOT)))
    new = not path.exists()
    with path.open("a", newline="") as f:
        out = csv.DictWriter(f, FIELDS, delimiter="\t", lineterminator="\n")
        if new:
            out.writeheader()
        out.writerow(rec)


def read_runs(key):
    with tsv(key).open() as f:
        recs = list(csv.DictReader(f, delimiter="\t"))
    whole_run = ROWS[key].tests[0].profile == TOLERANT
    return [dict(r, passed=passed_for(key, r["status"], r["red_rows"], whole_run), seconds=float(r["seconds"]), load_max=float(r["load_max"]),
                 qualifies=r["qualifies"] == "yes") for r in recs]


def cmd_rows(_):
    for key, row in ROWS.items():
        print(f"{key}\t{row.rule[0]} {row.rule[1]:.3f}")
        for t in row.tests:
            print(f"\t{t.id} (profile {t.profile}{', ignored' if t.ignored else ''})")


def cmd_run(args):
    for var in ("FLOW_JOB_ID", "FLOW_JOB_LOG"):
        if var not in os.environ:
            raise SystemExit(f"known-red-gate-load: run only inside `flow job run --queue` ({var} is unset)")
    w = walls()
    adm = admission(w, os.environ["FLOW_JOB_LOG"])
    floor, p90, _ = gate_loads(w, datetime.now(timezone.utc))
    for key, test in ((k, t) for k in args.keys for t in ROWS[k].tests):
        rec = run_once(w, key, test, adm, floor, p90)
        if rec is None:
            continue
        print("\t".join(rec[k] for k in FIELDS), flush=True)
        for owner, row in ROWS.items():
            if test in row.tests:
                append(owner, rec)


TIME_L = re.compile(r"^\s*([0-9.]+) real\s+([0-9.]+) user\s+([0-9.]+) sys\s*$")
IVCSW = re.compile(r"^\s*(\d+)\s+involuntary context switches\s*$")
FRAME = re.compile(r"^([ +!:|]+)(\d+) (.*)$")  # `sample` draws its call tree with `+ ! : |`
LOCK_LEAVES = ("__psynch_mutexwait", "__ulock_wait")
WAIT_LEAVES = ("__psynch_cvwait", "kevent", "semaphore_wait_trap", "semaphore_timedwait_trap", "__semwait_signal",
               "mach_msg", "__select", "__workq_kernreturn", "poll", "__wait4", "__recvfrom", "read", "nanosleep")


def rusage(text):
    """(wall, cpu, involuntary switches) from the `/usr/bin/time -l` lines of `text`; cpu = user + sys."""
    times = [m for m in map(TIME_L.match, text.splitlines()) if m]
    switches = [m for m in map(IVCSW.match, text.splitlines()) if m]
    if len(times) != 1 or len(switches) != 1:
        raise SystemExit(f"known-red-gate-load: {len(times)} `real user sys` and {len(switches)} switch lines, not one")
    real, user, sys_ = map(float, times[0].groups())
    return real, user + sys_, int(switches[0].group(1))


def leaf_class(frame):
    name = frame.split("  (in ")[0].strip()
    if name.startswith(LOCK_LEAVES):
        return "lock"
    if name.startswith(WAIT_LEAVES):
        return "wait"
    return "cpu"


def thread_samples(report):
    """{thread: {"lock"|"wait"|"cpu": samples}} of one `sample` report: each frame's own samples (its count less its
    children's) go to the class of that frame, so a thread blocked in a mutex counts as lock, in a condvar as wait."""
    lines = report.split("Call graph:\n", 1)[1].split("\n\n", 1)[0].splitlines()
    out, stack, thread = {}, [], None  # stack: [indent, count, children's count, frame]

    def close(depth):
        while stack and stack[-1][0] >= depth:
            indent, count, children, frame = stack.pop()
            if indent > 4 and count > children:
                cls = out[thread]
                cls[leaf_class(frame)] = cls.get(leaf_class(frame), 0) + count - children

    for m in filter(None, map(FRAME.match, lines)):
        indent, count, frame = len(m.group(1)), int(m.group(2)), m.group(3)
        close(indent)
        if indent == 4:
            thread = frame
            out.setdefault(thread, {})
        else:
            stack[-1][2] += count
        stack.append([indent, count, 0, frame])
    close(0)
    return out


def descendants(pid):
    kids = subprocess.run(["pgrep", "-P", str(pid)], capture_output=True, text=True).stdout.split()
    return [d for k in kids for d in (k, *descendants(k))]


def sample_until_exit(run, name, prefix):
    """`sample` the test process below the process `run` (nextest starts it as `<binary> <name> --exact ...`) in 2-s
    chunks until it exits; returns the chunk reports, at least one."""
    pid = None
    while pid is None and run.poll() is None:
        args = {d: subprocess.run(["ps", "-o", "args=", "-p", d], capture_output=True, text=True).stdout.strip()
                for d in descendants(run.pid)}
        pid = next((d for d, a in args.items() if {name, "--exact"} <= set(a.split())), None)
        if pid is None:
            subprocess.run(["sleep", "0.2"], check=True)
    if pid is None:
        raise SystemExit(f"known-red-gate-load: {name}: no process `{name} --exact` appeared below pid {run.pid} before "
                         f"it exited; no sample report for {prefix}")
    reports = []
    for k in range(10_000):
        path = prefix.with_name(f"{prefix.name}.sample-{k:03d}.txt")
        done = subprocess.run(["sample", pid, "2", "-file", path], capture_output=True, text=True)
        if not path.exists() or path.stat().st_size == 0:
            if subprocess.run(["kill", "-0", pid], capture_output=True).returncode != 0:
                if not reports:
                    raise SystemExit(f"known-red-gate-load: {name}: pid {pid} exited before its first sample report "
                                     f"{path}; no split for {prefix}")
                return reports  # the test exited during this chunk
            raise SystemExit(f"known-red-gate-load: {name}: `sample` exit {done.returncode} wrote no report {path} while "
                             f"pid {pid} still runs: {done.stderr.strip()}")
        reports.append(path)
    raise SystemExit(f"known-red-gate-load: {name} still runs after 10000 sample chunks")


def cause_once(w, key, test):
    """One cause run of `test`: its seconds split into CPU (user+sys of `/usr/bin/time -l`, less the same cost of the
    `nextest list` that builds and lists it) and off-CPU, and each thread's samples split into cpu, lock and wait by
    the frame each sample stopped in."""
    env = dict(os.environ, **dict(test.env))
    LOGS.mkdir(parents=True, exist_ok=True)
    listed = subprocess.run(["/usr/bin/time", "-l", *nextest(test), "--message-format", "oneline"], cwd=ROOT, env=env,
                            check=True, capture_output=True, text=True)
    if len([line for line in listed.stdout.splitlines() if line.strip()]) != 1:
        raise SystemExit(f"known-red-gate-load: {key}: the filter of {test.id} lists {listed.stdout!r}, not one test")
    _, list_cpu, _ = rusage(listed.stderr)
    load_start, started = load1(), datetime.now(timezone.utc)
    log = LOGS / f"{key}.cause.{os.environ['FLOW_JOB_ID']}.{started:%Y%m%dT%H%M%SZ}.log"
    with log.open("w") as out:
        run = subprocess.Popen(["/usr/bin/time", "-l", *nextest(test, "--no-fail-fast", "--retries", "0")], cwd=ROOT,
                               env=env, stdout=out, stderr=subprocess.STDOUT)
        reports = sample_until_exit(run, test.name, log)
        run.wait()
    ended = datetime.now(timezone.utc)
    lines = log.read_text(errors="replace").splitlines()
    body = lines[:next((i for i, line in enumerate(lines) if line.lstrip().startswith("Summary [")), len(lines))]
    statuses = [m for m in map(STATUS.match, body) if m]
    if len(statuses) != 1:
        raise SystemExit(f"known-red-gate-load: {log} holds {len(statuses)} test status lines, not one")
    status, seconds = statuses[0].group(1), float(statuses[0].group(2))
    _, run_cpu, switches = rusage("\n".join(lines))
    threads = {}
    for path in reports:
        for thread, classes in thread_samples(path.read_text(errors="replace")).items():
            for cls, n in classes.items():
                threads.setdefault(thread, {}).setdefault(cls, 0)
                threads[thread][cls] += n
    out = [f"cause run of {test.id}",
           f"job {os.environ['FLOW_JOB_ID']} started {started:%Y-%m-%dT%H:%M:%SZ} ended {ended:%Y-%m-%dT%H:%M:%SZ} "
           f"load_start {load_start:.2f} load_max {load_max(w, started, ended, load_start):.2f}",
           f"status {status} seconds {seconds:.3f} log {log.relative_to(ROOT)}",
           f"cpu {run_cpu - list_cpu:.2f} s (run {run_cpu:.2f} less list {list_cpu:.2f}) = "
           f"{(run_cpu - list_cpu) / seconds:.2f} of the test's seconds; involuntary switches {switches}",
           f"sample reports {len(reports)} (2 s each); per thread, samples by the frame they stopped in:"]
    for thread, classes in sorted(threads.items(), key=lambda t: -sum(t[1].values())):
        out.append(f"  {sum(classes.values()):7d}  cpu {classes.get('cpu', 0):7d}  lock {classes.get('lock', 0):7d}"
                   f"  wait {classes.get('wait', 0):7d}  {thread}")
    log.with_name(log.stem + ".cause.txt").write_text("\n".join(out) + "\n")
    print("\n".join(out), flush=True)


def cmd_cause(args):
    if "FLOW_JOB_ID" not in os.environ:
        raise SystemExit("known-red-gate-load: cause only inside `flow job run --queue` (FLOW_JOB_ID is unset)")
    w = walls()
    for key, test in ((k, t) for k in args.keys for t in ROWS[k].tests):
        cause_once(w, key, test)


def cmd_band(_):
    floor, p90, n = gate_loads(walls(), datetime.now(timezone.utc))
    print(f"gate band (last {GATE_DAYS} days, {'+'.join(GATE_KINDS)} jobs): n={n} median={floor:.2f} p90={p90:.2f}")


def cmd_verdict(args):
    for key in args.keys or ROWS:
        runs = read_runs(key) if tsv(key).exists() else []
        by_test = {t.id: [r for r in runs if r["test"] == t.id] for t in ROWS[key].tests}
        p90 = float(runs[-1]["p90"]) if runs else math.inf
        try:
            print(f"{key}\t{row_verdict(ROWS[key].rule, by_test, p90)}")
        except NoVerdict as e:
            print(f"{key}\tno verdict: {e}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("rows").set_defaults(fn=cmd_rows)
    sub.add_parser("band").set_defaults(fn=cmd_band)
    r = sub.add_parser("run")
    r.add_argument("keys", nargs="+", choices=ROWS)
    r.set_defaults(fn=cmd_run)
    c = sub.add_parser("cause")
    c.add_argument("keys", nargs="+", choices=ROWS)
    c.set_defaults(fn=cmd_cause)
    v = sub.add_parser("verdict")
    v.add_argument("keys", nargs="*")
    v.set_defaults(fn=cmd_verdict)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
