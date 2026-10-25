#!/usr/bin/env python3
"""Collect unused directories from cargo's per-unit build tree.

Two collectors, both dry runs unless --apply:

Build hash dirs, ``target/<profile>/build/<crate>/<metadata-hash>/``. The
metadata hash covers the resolved feature set, so two gates that unify features
differently mint two directories for the SAME target and neither is ever
reclaimed. A directory is garbage when ``--keep`` newer directories for the same
(profile, crate, target) exist. Grouping is by the target identity cargo writes
into ``fingerprint/`` (``lib-reqwest``, ``test-integration-test-loro_suite``,
...); a directory whose identity cannot be read forms its own group and is never
collected. Age alone never condemns a hash dir: cargo writes nothing into the
dir of a fresh unit, so a dir the last build used can look years old.

Incremental dirs, ``target/debug/incremental/<crate>-<hash>/`` (``--incremental``).
Mark and sweep: ``--mark`` at the start of a build moves ``.gc-mark`` to
``.gc-mark.prev`` and touches a new ``.gc-mark``; the sweep after the build
deletes every dir whose newest entry is older than ``.gc-mark.prev`` and that is
not among the ``--keep`` newest dirs of its unit name, so what the last two
builds compiled survives. Deleting one costs a non-incremental compile of that
unit the next time it changes, never a stale build. Known limit: a fresh unit
writes nothing into its dir, so the markers do not protect it; at keep 2 the
first CANON one-line edit after a seed sweep measured about +170 s (test
profile, 3 noisy runs: +318, +1, +192 s). So the incremental sweep runs only
under disk pressure (``--incremental-below``) or at a seed refresh
(``--incremental``).

Refuses with exit 3 while any process holds a file open under a profile dir.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys


PROFILES = ("debug", "release")
HASH_SUFFIX = re.compile(r"-[0-9a-f]{8,}$")
MARK = ".gc-mark"
PREV_MARK = ".gc-mark.prev"

EXIT_BUSY = 3


class Unit:
    """One <crate>/<hash> directory: what it is and when it was built."""

    def __init__(self, profile, crate, path):
        self.profile = profile
        self.crate = crate
        self.path = path
        self.hash = os.path.basename(path)
        self.target = _target_identity(path) or self.hash
        self.mtime = _built_at(path)

    @property
    def group(self):
        return (self.profile, self.crate, self.target)


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


def tree_usage(path):
    """(bytes, files) under path."""
    total = files = 0
    for root, _dirs, names in os.walk(path):
        for name in names:
            total += os.lstat(os.path.join(root, name)).st_size
            files += 1
    return total, files


def used_since(path, cutoff):
    """True when path or any entry below it was modified at or after cutoff."""
    if os.lstat(path).st_mtime >= cutoff:
        return True
    for root, dirs, names in os.walk(path):
        for name in dirs + names:
            if os.lstat(os.path.join(root, name)).st_mtime >= cutoff:
                return True
    return False


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
                    units.append(Unit(profile, crate, unit_dir))
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


def mark(target_dir):
    current = os.path.join(target_dir, MARK)
    if os.path.exists(current):
        os.replace(current, os.path.join(target_dir, PREV_MARK))
    with open(current, "w"):
        pass


def sweep_cutoff(target_dir):
    """mtime of the older of the two markers; the sweep keeps what is newer."""
    for name in (PREV_MARK, MARK):
        path = os.path.join(target_dir, name)
        if os.path.exists(path):
            return os.stat(path).st_mtime
    raise SystemExit(
        "no %s in %s: run `target-gc.py %s --mark` at the start of the build"
        % (MARK, target_dir, target_dir)
    )


def incremental_dirs(target_dir):
    root = os.path.join(target_dir, "debug", "incremental")
    if not os.path.isdir(root):
        return []
    paths = (os.path.join(root, name) for name in sorted(os.listdir(root)))
    return [p for p in paths if os.path.isdir(p)]


def newest_entry(path):
    newest = os.lstat(path).st_mtime
    for root, dirs, names in os.walk(path):
        for name in dirs + names:
            newest = max(newest, os.lstat(os.path.join(root, name)).st_mtime)
    return newest


def incremental_beyond_keep(dirs, keep):
    """Dirs that are not among the newest ``keep`` of their unit name (``<name>-<hash>``)."""
    by_name = {}
    for path in dirs:
        by_name.setdefault(os.path.basename(path).rsplit("-", 1)[0], []).append(path)
    out = []
    for members in by_name.values():
        if len(members) > keep:
            out += sorted(members, key=newest_entry, reverse=True)[keep:]
    return sorted(out)


def holders(target_dir):
    """(pid, command, path) of every open file under a profile dir of target_dir."""
    roots = tuple(os.path.join(os.path.realpath(target_dir), p) + os.sep for p in PROFILES)
    # -F pcn prints pid, command name and file name only; never argv or environment.
    probe = subprocess.run(["lsof", "-w", "-n", "-P", "-F", "pcn"], capture_output=True, text=True)
    if probe.returncode != 0:
        raise SystemExit("lsof failed (exit %d): %s" % (probe.returncode, probe.stderr.strip()))
    found = []
    pid = comm = None
    for line in probe.stdout.splitlines():
        tag, value = line[:1], line[1:]
        if tag == "p":
            pid = int(value)
        elif tag == "c":
            comm = value
        elif tag == "n" and value.startswith(roots) and pid != os.getpid():
            found.append((pid, comm, value))
    return found


def human(n):
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if abs(n) < 1024 or unit == "TB":
            return "%.1f %s" % (n, unit)
        n /= 1024.0


def remove(path, apply_):
    size, files = tree_usage(path)
    if apply_:
        shutil.rmtree(path)
    return size, files


def free_gib(path):
    return shutil.disk_usage(path).free / 1024 ** 3


def run(target_dir, keep, incremental, below_gib, apply_):
    if not os.path.isdir(target_dir):
        raise SystemExit("no such target dir: %s" % target_dir)
    if incremental or below_gib is not None:
        sweep_cutoff(target_dir)

    busy = holders(target_dir)
    if busy:
        sys.stderr.write("refusing to run: files under %s are open:\n" % target_dir)
        for pid, comm, path in busy[:20]:
            sys.stderr.write("  %s(%d) %s\n" % (comm, pid, path))
        return EXIT_BUSY

    verb = "rm" if apply_ else "--"
    totals = {}

    units = collect_units(target_dir)
    groups = plan(units, keep)
    freed = files = 0
    for (profile, crate, target), survivors, doomed in groups:
        print("%s/%-30s %-46s %2d superseded" % (profile, crate, target, len(doomed)))
        newest = survivors[0].mtime if survivors else 0
        for unit in doomed:
            size, n = remove(unit.path, apply_)
            freed += size
            files += n
            behind = (newest - unit.mtime) / 3600.0
            print("    %s %s  %8s  %6.1fh behind newest" % (verb, unit.hash, human(size), behind))
        for unit in survivors:
            print("    keep %s" % unit.hash)
    totals["build hash dirs"] = (sum(len(d) for _g, _s, d in groups), len(units), freed, files)

    if below_gib is not None:
        free = free_gib(target_dir)
        incremental = free < below_gib
        print("incremental sweep: %s, %.0f GiB free, threshold %g GiB" % ("runs" if incremental else "skipped", free, below_gib))
    if incremental:
        cutoff = sweep_cutoff(target_dir)
        dirs = incremental_dirs(target_dir)
        stale = [path for path in incremental_beyond_keep(dirs, keep) if not used_since(path, cutoff)]
        freed = files = 0
        for path in stale:
            size, n = remove(path, apply_)
            freed += size
            files += n
            print("%s %s  %8s" % (verb, path, human(size)))
        totals["incremental dirs"] = (len(stale), len(dirs), freed, files)

    print()
    for kind, (dirs, scanned, size, n) in totals.items():
        print(
            "%-17s: %s %d of %d dirs, %s (%d B) in %d files"
            % (kind, "freed" if apply_ else "would free", dirs, scanned, human(size), size, n)
        )
    if not apply_:
        print("dry run -- pass --apply to delete")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("target_dir", nargs="?", default="target")
    ap.add_argument("--keep", type=int, default=2, help="newest dirs kept per build target and per incremental unit name")
    ap.add_argument("--incremental", action="store_true",
                    help="also sweep debug/incremental dirs older than the markers")
    ap.add_argument("--incremental-below", type=float, metavar="GIB",
                    help="sweep debug/incremental only when less than GIB is free after the build hash sweep")
    ap.add_argument("--mark", action="store_true", help="rotate the markers and exit")
    ap.add_argument("--apply", action="store_true", help="delete (default: dry run)")
    args = ap.parse_args()

    if args.mark:
        mark(args.target_dir)
        return 0
    if args.incremental and args.incremental_below is not None:
        ap.error("--incremental and --incremental-below exclude each other")
    return run(args.target_dir, args.keep, args.incremental, args.incremental_below, args.apply)


if __name__ == "__main__":
    sys.exit(main())
