#!/usr/bin/env python3
"""Collect unused directories from cargo's per-unit build tree.

Two collectors, both dry runs unless --apply:

Build hash dirs, ``target/<profile>/build/<crate>/<metadata-hash>/``. The
metadata hash covers the resolved feature set, so two gates that unify features
differently mint two directories for the SAME target and neither is ever
reclaimed. Age alone never condemns a hash dir: cargo writes nothing into the
dir of a fresh unit, so a dir the last build used can look years old. So the
pass first replays the cargo calls recorded in ``$HOLON_CARGO_RECORD`` (see
scripts/cargo-shape-env.sh) as ``--no-run`` builds with their recorded env and
takes the dirs of the units cargo reports as in use; those are never collected.
Without a record the pass is skipped. Of the unused dirs of a (profile, crate,
target) group, the newest survive until the group holds ``--keep`` dirs.
Grouping is by the target identity cargo writes into ``fingerprint/``
(``lib-reqwest``, ``test-integration-test-loro_suite``, ...); a directory whose
identity cannot be read forms its own group.

Below the disk floor (``--floor GIB``, the gates pass ``$GATE_DISK_FLOOR_GIB``)
the incremental sweep runs, and then the unused hash dirs that ``--keep`` kept
go too, oldest first, until the floor is free again.

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
below the floor or at a seed refresh
(``--incremental``).

Refuses with exit 3 while any process holds a file open under a profile dir.
"""

import argparse
import filecmp
import glob
import importlib.machinery
import importlib.util
import json
import os
import re
import shlex
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


def _shape_check():
    loader = importlib.machinery.SourceFileLoader(
        "check_compile_shape", os.path.join(os.path.dirname(os.path.realpath(__file__)), "check-compile-shape.py"))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


# Options that pick which targets a build compiles and in which cargo profile; with the build options they make a
# replay of a nextest call build what the call built and nothing else.
TARGET_SELECTION = {"--test": 1, "--bin": 1, "--example": 1, "--bench": 1, "--lib": 0, "--bins": 0, "--tests": 0,
                    "--examples": 0, "--benches": 0, "--all-targets": 0, "--doc": 0}


def replay_argv(check, argv):
    """The `cargo` argv that builds what the recorded call built and runs nothing, or None for a call that builds
    nothing. A clippy call keeps its lint args: another lint set reruns every workspace member's clippy unit."""
    sub, args = check.subcommand(argv)
    args, after = (args[:args.index("--")], args[args.index("--"):]) if "--" in args else (args, [])
    if sub == "nextest":
        if not args or args[0] not in check.NEXTEST_COMPILING or check.NEXTEST_REUSE & {a.partition("=")[0] for a in args}:
            return None
        sub, args = "test", cargo_options(check, args[1:])
    if sub not in {"test", "check", "clippy", "build"}:
        return None
    return [sub, *args, *(["--no-run"] if sub == "test" and "--no-run" not in args else []), "--message-format=json",
            *(after if sub == "clippy" else [])]


def cargo_options(check, args):
    """The cargo build and target-selection options of a nextest call; nextest's own options (its `--profile`
    too) are dropped, and its `--cargo-profile` is cargo's `--profile`."""
    from_nextest = {"--cargo-profile": "--profile"}
    arities = {**check.BUILD_OPTIONS, **TARGET_SELECTION, "--profile": 1}
    out, i = [], 0
    while i < len(args):
        name, eq, _ = args[i].partition("=")
        arity = None if name == "--profile" else arities.get(from_nextest.get(name, name))
        if name.startswith("-p") and not name.startswith("--") and len(name) > 2:
            out.append(args[i])
        elif arity is not None:
            take = 1 if eq or not arity else 2
            out += [from_nextest.get(name, name) + args[i][len(name):], *args[i + 1:i + take]]
            i += take - 1
        i += 1
    return out


def in_use(target_dir, record):
    """The hash dirs of target_dir that the recorded cargo calls use, from cargo's own artifact report."""
    check = _shape_check()
    calls = check.load(record)
    if not calls:
        raise SystemExit("HOLON_CARGO_RECORD=%s holds no cargo call: the in-use units are unknown" % record)
    target = os.path.realpath(target_dir)
    shim = os.path.realpath(os.path.join(os.path.dirname(os.path.realpath(__file__)), "cargo-shape"))
    path = os.pathsep.join(d for d in os.environ["PATH"].split(os.pathsep) if os.path.realpath(d) != shim)
    replays, skipped = {}, {}
    for call in calls:
        argv = replay_argv(check, call["argv"])
        if argv is None:
            skipped.setdefault(shlex.join(["cargo", *call["argv"]]), "builds nothing")
            continue
        hashed = [n for n, v in call["env"].items() if isinstance(v, str) and v.startswith("sha256:")]
        if hashed:
            raise SystemExit("cannot replay cargo %s: the record holds only a hash of %s" % (" ".join(call["argv"]), hashed))
        replays.setdefault(json.dumps([argv, sorted(call["env"].items())]), (argv, call))
    used, built, replayed = set(), 0, {}
    for argv, call in replays.values():
        env = {**os.environ, "PATH": path}
        for name, value in call["env"].items():
            if value is None:
                env.pop(name, None)
            else:
                env[name] = value
        shown = shlex.join(["cargo", *call["argv"]])
        elsewhere = manifest_target_dir(argv, env, os.path.dirname(target))
        if elsewhere not in (None, target):
            skipped.setdefault(shown, "builds into %s, whose hash dirs this pass never collects" % elsewhere)
            continue
        replayed[shown] = True
        out = subprocess.run(["cargo", *argv], cwd=os.path.dirname(target), env=env, capture_output=True, text=True)
        if out.returncode != 0:
            raise SystemExit("replay `cargo %s` failed (exit %d):\n%s" % (" ".join(argv), out.returncode, out.stderr[-3000:]))
        for line in out.stdout.splitlines():
            msg = json.loads(line)
            paths = msg.get("filenames", []) if msg["reason"] == "compiler-artifact" else \
                [msg["out_dir"]] if msg["reason"] == "build-script-executed" else []
            built += msg["reason"] == "compiler-artifact" and not msg["fresh"]
            for p in paths:
                unit = _unit_of(target, os.path.realpath(p))
                if unit:
                    used.add(unit)
    for shown in replayed:
        print("replayed: %s" % shown)
    for shown, reason in skipped.items():
        print("not replayed (%s): %s" % (reason, shown))
    print("in use: %d hash dirs from %d replayed call(s) of %s; the replay built %d unit(s)"
          % (len(used), len(replayed), record, built))
    return used


def manifest_target_dir(argv, env, cwd):
    """The real path of the target dir that the --manifest-path of `cargo argv` builds into, by cargo's own answer
    (an out-of-workspace manifest has a target dir of its own), or None for a call without one."""
    args = argv[:argv.index("--")] if "--" in argv else argv
    manifest = next((a.partition("=")[2] or args[i + 1] for i, a in enumerate(args)
                     if a.partition("=")[0] == "--manifest-path"), None)
    if manifest is None:
        return None
    out = subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", manifest],
                         cwd=cwd, env=env, capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit("cargo metadata --manifest-path %s failed (exit %d):\n%s" % (manifest, out.returncode, out.stderr[-3000:]))
    return os.path.realpath(json.loads(out.stdout)["target_directory"])


def _unit_of(target, path):
    """target/<profile>/build/<crate>/<hash> for a path below it; None for a path of another target triple or
    target dir, whose units this script never collects."""
    parts = os.path.relpath(path, target).split(os.sep)
    if parts[0] not in PROFILES:
        return None
    if parts[1] != "build":
        return _copied_up_from(target, parts[0], path)
    if len(parts) < 5:
        raise SystemExit("cargo reported %s, outside any build hash dir: its unit dir is unknown" % path)
    return os.path.join(target, *parts[:4])


def _copied_up_from(target, profile, path):
    """The hash dir whose out/ holds the file that cargo copied up to `path` (a bin or an example); its name there
    spells `-` as `_`. Older hash dirs of the same target hold same-named files, so only the content tells."""
    name = os.path.basename(path)
    found = [p for n in sorted({name, name.replace("-", "_")})
             for p in glob.glob(os.path.join(target, profile, "build", "*", "*", "out", glob.escape(n)))
             if os.path.getsize(p) == os.path.getsize(path) and filecmp.cmp(p, path, shallow=False)]
    if len(found) != 1:
        raise SystemExit("cargo reported %s, a copy of %d file(s) under build hash dirs %s: its unit dir is unknown"
                         % (path, len(found), found))
    return os.path.dirname(os.path.dirname(found[0]))


def plan(units, keep, used):
    """Return [(group, survivors, doomed)] for every group that loses a member. A used dir survives and counts
    toward ``keep``."""
    groups = {}
    for unit in units:
        groups.setdefault(unit.group, []).append(unit)
    out = []
    for group, members in sorted(groups.items()):
        members.sort(key=lambda u: u.mtime, reverse=True)
        mine = [u for u in members if os.path.realpath(u.path) in used]
        rest = [u for u in members if os.path.realpath(u.path) not in used]
        room = max(0, keep - len(mine))
        if rest[room:]:
            out.append((group, mine + rest[:room], rest[room:]))
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


def run(target_dir, keep, incremental, floor_gib, apply_):
    if not os.path.isdir(target_dir):
        raise SystemExit("no such target dir: %s" % target_dir)
    if incremental or floor_gib is not None:
        sweep_cutoff(target_dir)

    busy = holders(target_dir)
    if busy:
        sys.stderr.write("refusing to run: files under %s are open:\n" % target_dir)
        for pid, comm, path in busy[:20]:
            sys.stderr.write("  %s(%d) %s\n" % (comm, pid, path))
        return EXIT_BUSY

    verb = "rm" if apply_ else "--"
    totals = {}

    record = os.environ.get("HOLON_CARGO_RECORD")
    used = in_use(target_dir, record) if record else None
    if used is None:
        print("build hash dirs: skipped, HOLON_CARGO_RECORD is unset, so the dirs the last build uses are unknown")
    units = collect_units(target_dir) if used is not None else []
    groups = plan(units, keep, used)
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

    below = False
    if floor_gib is not None:
        free = free_gib(target_dir)
        below = incremental = free < floor_gib
        print("below the floor: %s, %.0f GiB free, floor %g GiB" % ("yes" if below else "no", free, floor_gib))
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
    if below and used is not None:
        doomed = {u.path for _g, _s, d in groups for u in d}
        spare = sorted((u for u in units if u.path not in doomed and os.path.realpath(u.path) not in used),
                       key=lambda u: u.mtime)
        # Counted in apparent bytes, not re-read from the disk, so a dry run reports what --apply deletes.
        need = (floor_gib - free_gib(target_dir)) * 1024 ** 3
        freed = files = gone = 0
        for unit in spare:
            if freed >= need:
                break
            size, n = remove(unit.path, apply_)
            freed += size
            files += n
            gone += 1
            print("%s %s  %8s" % (verb, unit.path, human(size)))
        totals["unused below floor"] = (gone, len(spare), freed, files)

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
    ap.add_argument("--floor", type=float, metavar="GIB",
                    help="below GIB free after the build hash pass: sweep debug/incremental, then unused hash dirs")
    ap.add_argument("--mark", action="store_true", help="rotate the markers and exit")
    ap.add_argument("--apply", action="store_true", help="delete (default: dry run)")
    args = ap.parse_args()

    if args.mark:
        mark(args.target_dir)
        return 0
    if args.incremental and args.floor is not None:
        ap.error("--incremental and --floor exclude each other")
    return run(args.target_dir, args.keep, args.incremental, args.floor, args.apply)


if __name__ == "__main__":
    sys.exit(main())
