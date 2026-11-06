#!/usr/bin/env python3
"""Self-check for scripts/target-gc.py on small fake target dirs.

Run: `python3 scripts/test-target-gc.py`. Exits non-zero on the first violation.
TARGET_GC=<path> runs the checks against another copy of the script. SEED=<n> replays the generated cases.
"""
import json
import os
import random
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


def gc(target, *args, env=None):
    return subprocess.run(
        [sys.executable, str(SCRIPT), str(target), *args], capture_output=True, text=True, env=env
    )


FAKE_CARGO = """#!/usr/bin/python3
import json, os, sys
with open(os.environ["FAKE_CARGO_LOG"], "a") as f:
    f.write(json.dumps({"argv": sys.argv[1:], "sha": os.environ.get("HOLON_BUILD_SHA")}) + "\\n")
sys.stdout.write(open(os.environ["FAKE_CARGO_MESSAGES"]).read())
"""


RECORDED = [["test", "--workspace", "--", "--ignored"],
            ["nextest", "run", "--profile", "load-tolerant", "--workspace", "--cargo-profile", "dev", "-E", "test(x)",
             "--no-fail-fast", "--test", "t"],
            ["check", "--target", "wasm32-unknown-unknown", "-p", "holon-web"],
            ["fmt", "--check"]]
REPLAYED = [["test", "--workspace", "--no-run", "--message-format=json"],
            ["test", "--workspace", "--profile", "dev", "--test", "t", "--no-run", "--message-format=json"],
            ["check", "--target", "wasm32-unknown-unknown", "-p", "holon-web", "--message-format=json"]]


def recorded_build(target, in_use, rng=None, uplifted=()):
    """The env of a gc run after a recorded build whose replay reports the units in `in_use`: as a compiled
    artifact, as a build-script run (its out dir), as a bin cargo copied up to the profile dir (at most one per
    crate, as cargo has one such path), or (without rng) as an artifact and a run."""
    tools = target / "fake-tools"
    tools.mkdir()
    (tools / "cargo").write_text(FAKE_CARGO)
    (tools / "cargo").chmod(0o755)
    record = target / "calls.jsonl"
    env = {"HOLON_BUILD_SHA": "0123abcd", "HOLON_BUILD_TIME": None}
    record.write_text("".join(json.dumps({"argv": argv, "env": env}) + "\n" for argv in RECORDED))
    messages = []
    for unit in in_use:
        kinds = [rng.choice(["artifact", "run", "copied"])] if rng else ["artifact", "run"]
        copied = unit.parents[2] / unit.parent.name
        if "copied" in kinds and copied.exists():
            kinds = ["run"]
        if "copied" in kinds:
            shutil.copy(unit / "out" / unit.parent.name.replace("-", "_"), copied)
            messages.append({"reason": "compiler-artifact", "fresh": True, "filenames": [str(copied)]})
        if "artifact" in kinds:
            messages.append({"reason": "compiler-artifact", "fresh": True,
                             "filenames": [str(next((unit / "out").iterdir()))]})
        if "run" in kinds:
            messages.append({"reason": "build-script-executed", "out_dir": str(unit / "out")})
    messages += [{"reason": "compiler-artifact", "fresh": True, "filenames": [str(p)]} for p in uplifted]
    messages.append({"reason": "build-finished", "success": True})
    (target / "messages.jsonl").write_text("".join(json.dumps(m) + "\n" for m in messages))
    return {**os.environ, "PATH": f"{tools}:{os.environ['PATH']}", "HOLON_CARGO_RECORD": str(record),
            "FAKE_CARGO_LOG": str(target / "cargo.log"), "FAKE_CARGO_MESSAGES": str(target / "messages.jsonl")}


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
    (unit / "out" / crate.replace("-", "_")).write_text(digest)
    age_tree(unit, when)
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
        r = gc(target, "--incremental", "--apply", "--keep", "1", env=recorded_build(target, []))
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
            r = gc(target, "--apply", "--keep", "1", "--floor", below, env=recorded_build(target, []))
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
        r = gc(target, "--keep", "1", "--apply", env=recorded_build(target, []))
        assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
        assert not old.exists(), "the superseded hash dir survived"
        assert new.exists(), "the newest hash dir was collected"
        assert rel.exists(), "a release hash dir was superseded by a debug one"
    finally:
        shutil.rmtree(target)


def check_in_use_never_collected():
    """Generated groups of hash dirs, some of them used by the recorded build: the pass keeps every used dir,
    and of the unused ones each group keeps only the newest it has room for under --keep."""
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    print(f"SEED={seed}")
    rng = random.Random(seed)
    # The used dir is older than its two unused siblings: what the wasm steps and a seed leave behind.
    examples = [[(3, {0})]]
    cases = examples + [[(rng.randint(1, 5), None) for _ in range(rng.randint(1, 3))] for _ in range(30)]
    for groups in cases:
        target = Path(tempfile.mkdtemp(prefix="target-gc-test-"))
        try:
            plan_, in_use = [], []
            for g, (n, used) in enumerate(groups):
                ages = rng.sample(range(100), n)
                used = used if used is not None else set(rng.sample(range(n), rng.randint(0, n)))
                units = [hash_dir(target, "debug", f"crate-{g}", f"{g:x}{i:015x}", f"bin-crate-{g}", OLD + 10 * age)
                         for i, age in enumerate(sorted(ages))]
                in_use += [units[i] for i in used]
                plan_.append((units, used))
            keep = 2 if groups is examples[0] else rng.choice([1, 2])
            env = recorded_build(target, in_use, rng)
            r = gc(target, "--apply", "--keep", str(keep), env=env)
            assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
            for units, used in plan_:
                for i in used:
                    assert units[i].exists(), f"collected a dir the recorded build uses (keep {keep}):\n{r.stdout}"
                unused = [u for i, u in enumerate(units) if i not in used]
                room = max(0, keep - len(used))
                want = unused[len(unused) - room:] if room else []
                assert [u for u in unused if u.exists()] == want, (
                    f"unused survivors {[u.name for u in unused if u.exists()]}, want {[u.name for u in want]}"
                    f" (keep {keep}, {len(used)} used):\n{r.stdout}")
            calls = [json.loads(line) for line in (target / "cargo.log").read_text().splitlines()]
            assert calls and all(c["sha"] == "0123abcd" for c in calls), f"the replay lost the recorded env: {calls}"
            assert [c["argv"] for c in calls] == REPLAYED, f"replayed {calls}, want {REPLAYED}"
        finally:
            shutil.rmtree(target)


def check_no_record_keeps_hash_dirs():
    target = Path(tempfile.mkdtemp(prefix="target-gc-test-"))
    try:
        old = hash_dir(target, "debug", "holon-app", "aaaaaaaaaaaaaaaa", "lib-holon_app", OLD)
        for i in range(3):
            hash_dir(target, "debug", "holon-app", f"b{i:015x}", "lib-holon_app", MARK + i)
        env = {k: v for k, v in os.environ.items() if k != "HOLON_CARGO_RECORD"}
        r = gc(target, "--keep", "1", "--apply", env=env)
        assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
        assert old.exists(), "collected a hash dir without knowing which dirs the last build uses"
        assert "HOLON_CARGO_RECORD" in r.stdout, f"the skipped hash pass is not disclosed:\n{r.stdout}"
    finally:
        shutil.rmtree(target)


def check_unknown_unit_fails_loud():
    """A bin cargo reports as copied up leaves its unit unknown when no file under a hash dir equals it, or when
    two do."""
    for equal_copies in (0, 2):
        target = Path(tempfile.mkdtemp(prefix="target-gc-test-"))
        try:
            old = hash_dir(target, "debug", "holon", "aaaaaaaaaaaaaaaa", "bin-holon", OLD)
            for i in range(2):
                hash_dir(target, "debug", "holon", f"b{i:015x}", "bin-holon", MARK + i)
            plant(target / "debug" / "holon", OLD)
            for unit in sorted((target / "debug" / "build" / "holon").iterdir())[:equal_copies]:
                shutil.copy(target / "debug" / "holon", unit / "out" / "holon")
            r = gc(target, "--keep", "1", "--apply", env=recorded_build(target, [], uplifted=[target / "debug" / "holon"]))
            assert r.returncode != 0, f"collected with an unknown unit ({equal_copies} equal copies):\n{r.stdout}"
            assert str(target / "debug" / "holon") in r.stderr, f"the failure does not name the artifact:\n{r.stderr}"
            assert old.exists(), "deleted with an unknown unit"
        finally:
                shutil.rmtree(target)


def check_hash_pass_only_below_floor():
    """Above the floor the unused dirs that --keep keeps survive; below it they go, oldest first, until the
    floor is free. The dirs are sparse files of 8 GiB apparent each, so what other processes write meanwhile stays
    far below one dir."""
    size = 8 * 1024 ** 3
    for extra, gone in ((None, 0), (1.5 * size, 2), (1e15, 3)):
        target = Path(tempfile.mkdtemp(prefix="target-gc-test-"))
        try:
            olds = [hash_dir(target, "debug", f"c{i}", "aaaaaaaaaaaaaaaa", f"lib-c{i}", OLD + 50 - 10 * i) for i in range(3)]
            news = [hash_dir(target, "debug", f"c{i}", "bbbbbbbbbbbbbbbb", f"lib-c{i}", MARK) for i in range(3)]
            for o in olds:
                with open(o / "out" / "big.bin", "wb") as f:
                    f.truncate(size)
                age_tree(o, os.stat(o).st_mtime)
            assert gc(target, "--mark").returncode == 0
            free = shutil.disk_usage(target).free
            floor = 0 if extra is None else (free + extra) / 1024 ** 3
            r = gc(target, "--apply", "--keep", "2", "--floor", repr(floor), env=recorded_build(target, news))
            assert r.returncode == 0, f"exit {r.returncode}: {r.stdout}{r.stderr}"
            assert all(n.exists() for n in news), "collected a used dir"
            order = [line.split()[1] for line in r.stdout.splitlines() if line.startswith("rm ")]
            assert order == [str(o) for o in reversed(olds)][:gone], f"floor +{extra} B: deleted {order}:\n{r.stdout}"
        finally:
            shutil.rmtree(target)


def main():
    checks = [check_in_use_never_collected, check_no_record_keeps_hash_dirs, check_hash_pass_only_below_floor,
              check_unknown_unit_fails_loud,
              check_marker_sweep, check_nothing_outside, check_refuses_while_open,
              check_dry_run, check_needs_marker, check_keep_newest_per_name,
              check_incremental_only_under_disk_pressure, check_mark_rotates, check_supersession,
              check_marker_keeps_dirs_beyond_keep]
    for check in checks:
        check()
        print(f"ok {check.__name__}")
    print("PASS")


if __name__ == "__main__":
    main()
