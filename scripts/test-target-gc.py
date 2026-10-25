#!/usr/bin/env python3
"""Self-check for scripts/target-gc.py on small fake target dirs.

Run: `python3 scripts/test-target-gc.py`. Exits non-zero on the first violation.
TARGET_GC=<path> runs the checks against another copy of the script.
"""
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCRIPT = Path(os.environ.get("TARGET_GC", REPO / "scripts" / "target-gc.py"))

OLD = 1_000_000_000
PREV_MARK = 2_000_000_000
MARK = 2_000_000_100


def gc(target, *args):
    return subprocess.run(
        [sys.executable, str(SCRIPT), str(target), *args], capture_output=True, text=True
    )


def plant(path, when, size=16):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"x" * size)
    os.utime(path, (when, when))


def age_tree(root, when):
    for dirpath, dirnames, filenames in os.walk(root):
        for name in dirnames + filenames:
            os.utime(os.path.join(dirpath, name), (when, when))
    os.utime(root, (when, when))


def hash_dir(target, profile, crate, digest, ident, when):
    unit = target / profile / "build" / crate / digest
    plant(unit / "fingerprint" / ident, when)
    plant(unit / "fingerprint" / "invoked.timestamp", when)
    plant(unit / "out" / f"lib{crate}-{digest}.rmeta", when, size=64)
    return unit


def fake_target():
    """A target as a gate leaves it, with both markers set; returns (target, paths)."""
    target = Path(tempfile.mkdtemp(prefix="target-gc-test-"))
    inc = target / "debug" / "incremental"
    p = {
        "stale": inc / "holon-0stale",
        "used_prev": inc / "holon-1usedprev",
        "used_deep": inc / "holon_app-2useddeep",
    }
    p["planted"] = [
        target / "debug" / "holon-gpui",
        target / "debug" / ".cargo-lock",
        target / "debug" / "examples" / "demo",
        target / "release" / "incremental" / "holon-0rel" / "s-r" / "dep-graph.bin",
        target / "wasm32-unknown-unknown" / "debug" / "incremental" / "holon-0w" / "s-w" / "x.bin",
        target / "gate-logs" / "gate-smoke.log",
        target / "nextest" / "archive.bin",
    ]
    for f in p["planted"]:
        plant(f, OLD)
    p["sole_hash"] = hash_dir(target, "debug", "memchr", "aaaaaaaaaaaaaaaa", "lib-memchr", OLD)
    for d in ("debug", "release", "wasm32-unknown-unknown", "gate-logs", "nextest"):
        age_tree(target / d, OLD)

    plant(p["stale"] / "s-a-working" / "dep-graph.bin", OLD, size=100)
    plant(p["stale"] / "s-a.lock", OLD, size=0)
    plant(p["used_prev"] / "s-b" / "query-cache.bin", PREV_MARK + 10)
    # A build that reused an old session and wrote one file deep inside it.
    plant(p["used_deep"] / "s-c" / "old.o", OLD)
    plant(p["used_deep"] / "s-c" / "work-products.bin", MARK + 10)
    for d in ("stale", "used_prev", "used_deep"):
        for sub in p[d].iterdir():
            if sub.is_dir():
                os.utime(sub, (OLD, OLD))
        os.utime(p[d], (OLD, OLD))

    plant(target / ".gc-mark.prev", PREV_MARK, size=0)
    plant(target / ".gc-mark", MARK, size=0)
    return target, p


def check_marker_sweep():
    target, p = fake_target()
    try:
        r = gc(target, "--incremental", "--apply", "--keep", "1")
        assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
        assert not p["stale"].exists(), "an incremental dir older than both markers survived"
        assert p["used_prev"].exists(), "an incremental dir the previous build used was swept"
        assert p["used_deep"].exists(), "an incremental dir with one file newer than the marker was swept"
    finally:
        shutil.rmtree(target)


def check_marker_keeps_dirs_beyond_keep():
    target, p = fake_target()
    newer = p["used_prev"].parent / "holon-3usedlast"
    plant(newer / "s-d" / "query-cache.bin", MARK + 20)
    os.utime(newer / "s-d", (OLD, OLD))
    os.utime(newer, (OLD, OLD))
    try:
        r = gc(target, "--incremental", "--apply", "--keep", "1")
        assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
        assert p["used_prev"].exists(), "a dir the previous marker build used was swept once it fell beyond --keep"
        assert not p["stale"].exists(), "an incremental dir older than both markers survived"
    finally:
        shutil.rmtree(target)


def check_nothing_outside():
    target, p = fake_target()
    try:
        r = gc(target, "--incremental", "--apply", "--keep", "1")
        assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
        for f in p["planted"]:
            assert f.exists(), f"swept a file outside debug/incremental and the build hash dirs: {f}"
        assert p["sole_hash"].exists(), "swept the only build hash dir of its crate and target"
    finally:
        shutil.rmtree(target)


def check_refuses_while_open():
    target, p = fake_target()
    held = p["stale"] / "s-a-working" / "dep-graph.bin"
    holder = subprocess.Popen(["/bin/sh", "-c", 'exec 3<"$1"; echo ready; exec sleep 60', "sh", str(held)],
                              stdout=subprocess.PIPE, text=True)
    try:
        assert holder.stdout.readline().strip() == "ready"
        r = gc(target, "--incremental", "--apply")
        assert r.returncode == 3, f"expected the busy exit 3, got {r.returncode}: {r.stdout}{r.stderr}"
        assert str(holder.pid) in r.stderr, f"the refusal does not name the holder:\n{r.stderr}"
        assert p["stale"].exists() and held.exists(), "deleted while a process held a file open"
    finally:
        holder.kill()
        holder.wait()
        shutil.rmtree(target)


def check_dry_run():
    target, p = fake_target()
    try:
        r = gc(target, "--incremental", "--keep", "1")
        assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
        assert p["stale"].exists(), "the dry run deleted"
        assert str(p["stale"]) in r.stdout, f"the dry run does not name the stale dir:\n{r.stdout}"
        assert "would free" in r.stdout and "100 B" in r.stdout and "2 files" in r.stdout, r.stdout
    finally:
        shutil.rmtree(target)


def check_needs_marker():
    target, p = fake_target()
    try:
        (target / ".gc-mark").unlink()
        (target / ".gc-mark.prev").unlink()
        r = gc(target, "--incremental", "--apply")
        assert r.returncode != 0, "swept without a marker"
        assert p["stale"].exists(), "deleted without a marker"
    finally:
        shutil.rmtree(target)


def check_mark_rotates():
    target = Path(tempfile.mkdtemp(prefix="target-gc-test-"))
    try:
        assert gc(target, "--mark").returncode == 0
        first = (target / ".gc-mark").stat().st_mtime
        os.utime(target / ".gc-mark", (first - 50, first - 50))
        assert gc(target, "--mark").returncode == 0
        assert (target / ".gc-mark.prev").stat().st_mtime == first - 50, "the old marker was not kept as .prev"
        assert (target / ".gc-mark").stat().st_mtime >= time.time() - 5, "the new marker is not fresh"
    finally:
        shutil.rmtree(target)


def check_keep_newest_per_name():
    target, p = fake_target()
    inc = target / "debug" / "incremental"
    olds = [inc / f"loro_suite-{i}old" for i in range(3)]
    for i, d in enumerate(olds):
        plant(d / "s-x" / "dep-graph.bin", OLD + 10 * i)
        os.utime(d / "s-x", (OLD + 10 * i, OLD + 10 * i))
        os.utime(d, (OLD + 10 * i, OLD + 10 * i))
    try:
        r = gc(target, "--incremental", "--apply", "--keep", "2")
        assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
        assert not olds[0].exists(), "the third-newest dir of a unit name survived keep 2"
        assert olds[1].exists() and olds[2].exists(), "one of the two newest dirs of a unit name was swept"
        assert p["stale"].exists(), "a stale dir among the newest two of its unit name was swept"
    finally:
        shutil.rmtree(target)


def check_incremental_only_under_disk_pressure():
    for below, swept in (("0", False), ("1e12", True)):
        target, p = fake_target()
        old = hash_dir(target, "debug", "holon-app", "aaaaaaaaaaaaaaaa", "test-integration-test-loro_suite", OLD)
        hash_dir(target, "debug", "holon-app", "bbbbbbbbbbbbbbbb", "test-integration-test-loro_suite", MARK)
        try:
            r = gc(target, "--apply", "--keep", "1", "--incremental-below", below)
            assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
            assert not old.exists(), f"the build hash sweep did not run at threshold {below} GiB"
            assert p["stale"].exists() != swept, (
                "the incremental sweep %s at threshold %s GiB" % ("did not run" if swept else "ran", below)
            )
        finally:
            shutil.rmtree(target)


def check_supersession():
    target = Path(tempfile.mkdtemp(prefix="target-gc-test-"))
    try:
        old = hash_dir(target, "debug", "holon-app", "aaaaaaaaaaaaaaaa", "test-integration-test-loro_suite", OLD)
        new = hash_dir(target, "debug", "holon-app", "bbbbbbbbbbbbbbbb", "test-integration-test-loro_suite", MARK)
        rel = hash_dir(target, "release", "holon-app", "cccccccccccccccc", "test-integration-test-loro_suite", OLD)
        r = gc(target, "--keep", "1", "--apply")
        assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
        assert not old.exists(), "the superseded hash dir survived"
        assert new.exists(), "the newest hash dir was collected"
        assert rel.exists(), "a release hash dir was superseded by a debug one"
    finally:
        shutil.rmtree(target)


def main():
    checks = [check_marker_sweep, check_nothing_outside, check_refuses_while_open,
              check_dry_run, check_needs_marker, check_keep_newest_per_name,
              check_incremental_only_under_disk_pressure, check_mark_rotates, check_supersession,
              check_marker_keeps_dirs_beyond_keep]
    for check in checks:
        check()
        print(f"ok {check.__name__}")
    print("PASS")


if __name__ == "__main__":
    main()
