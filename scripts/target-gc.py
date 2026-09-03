#!/usr/bin/env python3
"""Collect superseded directories from cargo's per-crate build tree.

Cargo lays intermediate artifacts out as
``target/<profile>/build/<crate>/<metadata-hash>/{fingerprint,out}``. The
metadata hash covers the resolved feature set, so two gates that unify features
differently mint two directories for the SAME target and neither is ever
reclaimed: this workspace reached 76 directories for one ``lib-reqwest`` and 878
for ``holon-app``'s 34 test targets, at ~238 MB apiece once a test binary links.

A directory is garbage exactly when a newer directory for the same (crate,
target) exists. Grouping is by the target identity cargo itself writes into
``fingerprint/`` (``lib-reqwest``, ``test-integration-test-loro_suite``, ...);
a directory whose identity cannot be read forms its own group and is never
collected.

``deps/`` is deliberately out of scope. One ``deps/`` entry is shared by every
target that links it, so supersession is not decidable from the entry alone --
cargo-sweep prunes there by age instead.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile


PROFILES = ("debug", "release")
HASH_SUFFIX = re.compile(r"-[0-9a-f]{8,}$")

EXIT_BUSY = 3


class Unit:
    """One <crate>/<hash> directory: what it is, when it was built, how big."""

    def __init__(self, crate, path):
        self.crate = crate
        self.path = path
        self.hash = os.path.basename(path)
        self.target = _target_identity(path) or self.hash
        self.mtime = _built_at(path)
        self.size = _tree_size(path)

    @property
    def group(self):
        return (self.crate, self.target)


def _target_identity(unit_dir):
    """Read the target name cargo stamped into fingerprint/, or None."""
    fingerprint = os.path.join(unit_dir, "fingerprint")
    names = []
    try:
        names = os.listdir(fingerprint)
    except OSError:
        pass
    for name in sorted(names):
        if name == "invoked.timestamp" or name.startswith("dep-"):
            continue
        if name.endswith(".json"):
            continue
        return name
    # No fingerprint: fall back to the artifact stem in out/, which carries the
    # same hash as the directory and must be stripped before it identifies a target.
    try:
        outs = sorted(os.listdir(os.path.join(unit_dir, "out")))
    except OSError:
        return None
    for name in outs:
        stem = name.split(".", 1)[0]
        stem = HASH_SUFFIX.sub("", stem)
        if stem:
            return "out:" + stem
    return None


def _built_at(unit_dir):
    stamp = os.path.join(unit_dir, "fingerprint", "invoked.timestamp")
    times = [os.stat(unit_dir).st_mtime]
    try:
        times.append(os.stat(stamp).st_mtime)
    except OSError:
        pass
    return max(times)


def _tree_size(path):
    total = 0
    for root, _dirs, files in os.walk(path):
        for name in files:
            try:
                total += os.lstat(os.path.join(root, name)).st_size
            except OSError:
                pass
    return total


def collect_units(target_dir):
    units = []
    for profile in PROFILES:
        build_root = os.path.join(target_dir, profile, "build")
        if not os.path.isdir(build_root):
            continue
        for crate in sorted(os.listdir(build_root)):
            crate_dir = os.path.join(build_root, crate)
            if not os.path.isdir(crate_dir):
                continue
            for entry in sorted(os.listdir(crate_dir)):
                unit_dir = os.path.join(crate_dir, entry)
                if os.path.isdir(unit_dir):
                    units.append(Unit(crate, unit_dir))
    return units


def plan(units, keep):
    """Return [(group, survivors, doomed)] for every group that loses a member."""
    groups = {}
    for unit in units:
        groups.setdefault(unit.group, []).append(unit)
    out = []
    for group, members in sorted(groups.items()):
        members.sort(key=lambda u: u.mtime, reverse=True)
        doomed = members[keep:]
        if doomed:
            out.append((group, members[:keep], doomed))
    return out


def busy_pids(target_dir):
    """PIDs of build processes that name this target dir in their argv."""
    probe = subprocess.run(
        ["pgrep", "-f", os.path.abspath(target_dir)],
        capture_output=True,
        text=True,
    )
    mine = {os.getpid(), os.getppid()}
    found = []
    for line in probe.stdout.split():
        pid = int(line)
        if pid in mine:
            continue
        comm = subprocess.run(
            ["ps", "-o", "comm=", "-p", str(pid)], capture_output=True, text=True
        ).stdout.strip()
        # comm only -- never `pgrep -fl` or `ps -o args`, which print the
        # process environment and have leaked API keys into transcripts before.
        base = os.path.basename(comm)
        if base.startswith(("cargo", "rustc", "sccache", "clang", "cc", "ld", "lld")):
            found.append((pid, base))
    return found


def human(n):
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if abs(n) < 1024 or unit == "TB":
            return "%.1f %s" % (n, unit)
        n /= 1024.0


def run(target_dir, keep, apply_):
    if not os.path.isdir(target_dir):
        raise SystemExit("no such target dir: %s" % target_dir)

    busy = busy_pids(target_dir)
    if busy:
        listing = ", ".join("%s(%d)" % (comm, pid) for pid, comm in busy)
        sys.stderr.write(
            "refusing to run: a build is using %s -- %s\n" % (target_dir, listing)
        )
        return EXIT_BUSY

    units = collect_units(target_dir)
    groups = plan(units, keep)

    freed = 0
    for (crate, target), survivors, doomed in groups:
        group_bytes = sum(u.size for u in doomed)
        freed += group_bytes
        print(
            "%-34s %-46s %2d superseded  %10s"
            % (crate, target, len(doomed), human(group_bytes))
        )
        newest = survivors[0].mtime if survivors else 0
        for unit in doomed:
            behind = (newest - unit.mtime) / 3600.0
            print(
                "    %s %s  %8s  %6.1fh behind newest"
                % ("rm" if apply_ else "--", unit.hash, human(unit.size), behind)
            )
            if apply_:
                shutil.rmtree(unit.path)
        for unit in survivors:
            print("    keep %s  %8s" % (unit.hash, human(unit.size)))

    print()
    print("units scanned : %d" % len(units))
    print("groups pruned : %d" % len(groups))
    print(
        "%-14s: %s%s"
        % (
            "freed" if apply_ else "would free",
            human(freed),
            "" if apply_ else "  (dry run -- pass --apply to delete)",
        )
    )
    return 0


def self_test():
    """Two hash dirs for one target, keep=1: exactly the older must go."""
    root = tempfile.mkdtemp(prefix="target-gc-selftest-")
    try:
        crate = os.path.join(root, "debug", "build", "holon-app")
        old = os.path.join(crate, "aaaaaaaaaaaaaaaa")
        new = os.path.join(crate, "bbbbbbbbbbbbbbbb")
        for unit, when in ((old, 1_000_000), (new, 2_000_000)):
            os.makedirs(os.path.join(unit, "fingerprint"))
            os.makedirs(os.path.join(unit, "out"))
            ident = os.path.join(unit, "fingerprint", "test-integration-test-loro_suite")
            with open(ident, "w") as fh:
                fh.write("x" * 16)
            stamp = os.path.join(unit, "fingerprint", "invoked.timestamp")
            open(stamp, "w").close()
            os.utime(stamp, (when, when))
            os.utime(unit, (when, when))

        units = collect_units(root)
        assert len(units) == 2, units
        assert {u.target for u in units} == {"test-integration-test-loro_suite"}, units

        groups = plan(units, keep=1)
        assert len(groups) == 1, groups
        (_group, survivors, doomed) = groups[0]
        assert [u.hash for u in survivors] == ["bbbbbbbbbbbbbbbb"], survivors
        assert [u.hash for u in doomed] == ["aaaaaaaaaaaaaaaa"], doomed

        rc = run(root, keep=1, apply_=True)
        assert rc == 0, rc
        assert not os.path.exists(old), "older dir survived"
        assert os.path.exists(new), "newer dir was collected"

        # Idempotent: a second pass finds nothing left to supersede.
        assert plan(collect_units(root), keep=1) == []
    finally:
        shutil.rmtree(root, ignore_errors=True)
    print("self-test PASS")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("target_dir", nargs="?", default="target")
    ap.add_argument("--keep", type=int, default=2, help="newest dirs kept per group")
    ap.add_argument("--apply", action="store_true", help="delete (default: dry run)")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()

    if args.self_test:
        return self_test()
    return run(args.target_dir, args.keep, args.apply)


if __name__ == "__main__":
    sys.exit(main())
