#!/usr/bin/env python3
"""Windowed GPUI gate: every holon-gpui test binary as its own process.

Usage: scripts/windowed-gate.py --features <cargo features> --log-dir <dir>
       [--jobs N] [--timeout SECONDS]

The windowed suite is every test binary of the holon-gpui package (the lib and
each `frontends/gpui/tests/*.rs` target; `windowed_log_capture` pins that each
one declares the shared `test_init`). Each binary runs alone under
`cargo nextest run --test-threads 1`, up to JOBS binaries at once, and is killed
(whole process group) after TIMEOUT seconds and reported HUNG.

A failing binary's nextest log is classified by scripts/keystone-known-reds.sh
against docs/Testing/KeystoneKnownReds.md (see docs/Testing/WindowedKnownReds.md):
all signatures registered -> KNOWN-RED (pass with note); anything else -> RED.

Exit 0: every binary PASS or KNOWN-RED. Exit 1: any RED or HUNG.
"""
import argparse
import json
import os
import signal
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CLASSIFIER = REPO / "scripts" / "keystone-known-reds.sh"


def default_jobs() -> int:
    return max(1, min(4, (os.cpu_count() or 4) // 4))


def build(features: str, log_dir: Path) -> list[str]:
    """Compile once; every later nextest call reuses the build via the two
    metadata files, so a binary's wall time is its tests, not a cargo re-check."""
    bm, cm = log_dir / "binaries-metadata.json", log_dir / "cargo-metadata.json"
    with open(bm, "wb") as fh:
        subprocess.run(["cargo", "nextest", "list", "--workspace", "--features", features,
                        "--list-type", "binaries-only", "--message-format", "json",
                        "--cargo-quiet"], cwd=REPO, check=True, stdout=fh)
    with open(cm, "wb") as fh:
        subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked"],
                       cwd=REPO, check=True, stdout=fh)
    return ["--binaries-metadata", str(bm), "--cargo-metadata", str(cm), "--workspace-remap", "."]


def nextest_args(reuse: list[str], filt: str) -> list[str]:
    return ["cargo", "nextest", "run", *reuse, "-E", filt, "--no-fail-fast",
            "--test-threads", "1", "--color", "never"]


def inventory(reuse: list[str]) -> list[tuple[str, int]]:
    out = subprocess.run(
        ["cargo", "nextest", "list", *reuse, "-E", "package(holon-gpui)",
         "--message-format", "json"],
        cwd=REPO, check=True, stdout=subprocess.PIPE, text=True).stdout
    suites = json.loads(out)["rust-suites"]
    found = []
    for binary_id, suite in suites.items():
        n = sum(1 for tc in suite["testcases"].values()
                if not tc.get("ignored") and tc.get("filter-match", {}).get("status") == "matches")
        if n:
            found.append((binary_id, n))
    return sorted(found)


def descendants(root: int) -> list[int]:
    """All transitive children of `root`, from one `ps` snapshot. nextest puts
    each test in its own process group, so a group kill of nextest misses them."""
    out = subprocess.run(["ps", "-axo", "pid=,ppid="], check=True,
                         stdout=subprocess.PIPE, text=True).stdout
    kids: dict[int, list[int]] = {}
    for line in out.splitlines():
        pid, ppid = map(int, line.split())
        kids.setdefault(ppid, []).append(pid)
    found, todo = [], [root]
    while todo:
        for kid in kids.get(todo.pop(), []):
            found.append(kid)
            todo.append(kid)
    return found


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def run_cmd(cmd: list[str], log: Path, timeout: int) -> tuple[int, bool, int]:
    """Returns (rc, hung, killed_descendants). On timeout the whole descendant
    tree is snapshotted BEFORE anything dies (killing nextest first would
    re-parent the tests to init and hide them), killed, and verified gone."""
    with open(log, "wb") as fh:
        proc = subprocess.Popen(cmd, cwd=REPO, stdout=fh, stderr=subprocess.STDOUT,
                                start_new_session=True)
        try:
            return proc.wait(timeout=timeout), False, 0
        except subprocess.TimeoutExpired:
            pass
        tree = descendants(proc.pid)
        for pid in tree:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait()
        deadline = time.monotonic() + 10
        while any(alive(p) for p in tree) and time.monotonic() < deadline:
            time.sleep(0.2)
        survivors = [p for p in tree if alive(p)]
        if survivors:
            raise RuntimeError(f"windowed-gate: processes survived the timeout kill: {survivors} ({log})")
        return -9, True, len(tree)


def run_one(binary_id: str, tests: int, reuse: list[str], log_dir: Path, timeout: int,
            suffix: str = "") -> dict:
    log = log_dir / (binary_id.replace("::", "__").replace("/", "_") + suffix + ".log")
    start = time.monotonic()
    rc, hung, killed = run_cmd(nextest_args(reuse, f"binary_id(={binary_id})"), log, timeout)
    wall = time.monotonic() - start
    if hung:
        verdict = "HUNG"
    elif rc == 0:
        verdict = "PASS"
    else:
        cls = subprocess.run([str(CLASSIFIER), str(log)], cwd=REPO,
                             stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        (log_dir / (log.stem + ".classified")).write_text(cls.stdout)
        verdict = "KNOWN-RED" if cls.returncode == 0 and "known-red:" in cls.stdout else "RED"
    return {"binary": binary_id, "tests": tests, "wall": wall, "verdict": verdict, "log": log,
            "orphans_killed": killed}


def settle(first: dict, rerun) -> dict:
    """A RED or HUNG binary is re-run once alone; a pass there makes it FLAKY."""
    if first["verdict"] not in ("RED", "HUNG"):
        return first
    second = rerun(first)
    second["first"] = first
    if second["verdict"] == "PASS":
        second["verdict"] = "FLAKY"
    return second


LEDGER = Path.home() / ".holon-gate" / "windowed-ledger.tsv"


def commit_id() -> str:
    out = subprocess.run(["jj", "--ignore-working-copy", "log", "-r", "@-", "--no-graph",
                          "-T", "commit_id.short()"], cwd=REPO, stdout=subprocess.PIPE, text=True)
    return out.stdout.strip() if out.returncode == 0 and out.stdout.strip() else "unknown"


def record_and_report(results: list[dict], ledger: Path = LEDGER) -> None:
    """Append FLAKY/RED/HUNG verdicts, then show the last 30 days' top flakes."""
    ledger.parent.mkdir(parents=True, exist_ok=True)
    today, commit = time.strftime("%Y-%m-%d"), commit_id()
    with open(ledger, "a") as fh:
        for r in results:
            if r["verdict"] in ("FLAKY", "RED", "HUNG"):
                logs = ",".join(str(x) for x in (r["first"]["log"], r["log"])) if "first" in r else str(r["log"])
                fh.write("\t".join([today, commit, r["binary"], r["verdict"], logs]) + "\n")
    cutoff = time.strftime("%Y-%m-%d", time.localtime(time.time() - 30 * 86400))
    flaky: dict[str, int] = {}
    for line in ledger.read_text().splitlines():
        date, _, binary, verdict, *_ = line.split("\t")
        if verdict == "FLAKY" and date >= cutoff:
            flaky[binary] = flaky.get(binary, 0) + 1
    if flaky:
        print(f"windowed-gate: top FLAKY binaries, last 30 days ({ledger}):")
        for binary, n in sorted(flaky.items(), key=lambda kv: -kv[1])[:5]:
            print(f"  {n:>3}x {binary}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--features", required=True)
    ap.add_argument("--log-dir", required=True, type=Path)
    ap.add_argument("--jobs", type=int, default=int(os.environ.get("WINDOWED_GATE_JOBS", default_jobs())))
    ap.add_argument("--timeout", type=int, default=int(os.environ.get("WINDOWED_GATE_TIMEOUT", 900)))
    args = ap.parse_args()
    args.log_dir.mkdir(parents=True, exist_ok=True)

    reuse = build(args.features, args.log_dir)
    binaries = inventory(reuse)
    if not binaries:
        print("windowed-gate: the holon-gpui inventory is empty — the filter is wrong", file=sys.stderr)
        return 1
    print(f"windowed-gate: {len(binaries)} binaries, {sum(n for _, n in binaries)} tests, "
          f"jobs={args.jobs}, per-binary timeout={args.timeout}s", flush=True)

    t0 = time.monotonic()
    results = []
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = [pool.submit(run_one, b, n, reuse, args.log_dir, args.timeout)
                   for b, n in binaries]
        for fut in futures:
            r = fut.result()
            results.append(r)
            print(f"{r['verdict']:<9} {r['binary']}  tests={r['tests']}  wall={r['wall']:.1f}s  log={r['log']}",
                  flush=True)
    suspects = [r for r in results if r["verdict"] in ("RED", "HUNG")]
    if suspects:
        print(f"windowed-gate: re-running {len(suspects)} red/hung binaries alone, once", flush=True)
    settled = []
    for r in results:
        r = settle(r, lambda f: run_one(f["binary"], f["tests"], reuse, args.log_dir,
                                        args.timeout, suffix=".rerun"))
        if "first" in r:
            print(f"{r['verdict']:<9} {r['binary']}  rerun wall={r['wall']:.1f}s  "
                  f"logs={r['first']['log']} {r['log']}", flush=True)
        settled.append(r)
    results = settled
    wall = time.monotonic() - t0

    serial = sum(r["wall"] for r in results)
    count = {v: sum(r["verdict"] == v for r in results)
             for v in ("PASS", "KNOWN-RED", "FLAKY", "RED", "HUNG")}
    bad = [r for r in results if r["verdict"] in ("RED", "HUNG")]
    print(f"windowed-gate: wall={wall:.1f}s serial-sum={serial:.1f}s " +
          " ".join(f"{k}={v}" for k, v in count.items()))
    for r in results:
        if r["verdict"] == "KNOWN-RED":
            print(f"  note: {r['binary']} is a registered known red "
                  f"(see {r['log'].with_suffix('.classified')})")
        elif r["verdict"] == "FLAKY":
            print(f"  note: {r['binary']} FLAKY: failed ({r['first']['verdict']}, {r['first']['log']}), "
                  f"passed alone on re-run ({r['log']})")
    for r in results:
        if r.get("orphans_killed") or r.get("first", {}).get("orphans_killed"):
            print(f"  note: ORPHAN-KILLED {r['binary']}: descendant processes outlived the timeout "
                  f"and were killed (all verified gone)")
    record_and_report(results)
    for r in bad:
        print(f"  FAIL: {r['verdict']} {r['binary']} (log {r['log']})", file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
