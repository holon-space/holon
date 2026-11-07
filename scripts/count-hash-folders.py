#!/usr/bin/python3
"""Count the hash dirs one gate used per unit, and fail on a unit built in a second compile shape.

    scripts/count-hash-folders.py <target dir> --mark <file> [--record <cargo-calls.jsonl>] [--baseline <file>]

A used dir is one built at or after the mark; with a record, also every dir a replay of the recorded
calls uses (scripts/target-gc.py `in_use`), so reused fresh units count too. Inside one package, one dir per cargo
profile hash (dev, test, build-override; check and build mode hash differently) is the single shape. Exit 1 on a
pair of dirs of the same hash that CANON calls (without a record: any call) use, naming the first crate down the
deps chain that differs in more than its deps, on a used dir no recorded call uses, and on a recorded clippy call
with lint args other than CLIPPY_LINTS: another lint set reruns a unit in its own dir, so no dir count shows it.
A pair is no second shape when one replayed call uses both dirs (cargo's own unit graph made both), or when only
check-mode calls (clippy, check) use one and only build-mode calls (test, nextest, build) the other and no
fingerprint field differs down the deps chain: the same unit in two modes, which cannot share artifacts. Dirs
only the EXCEPTIONS of scripts/check-compile-shape.py use are listed, not failed. A second shape whose key
(`pair_key`) the baseline (default scripts/compile-shape-baseline.txt) holds is listed, not failed, once per line.
"""
import argparse
import collections
import glob
import importlib.machinery
import importlib.util
import json
import os
import sys
import tempfile

SCRIPTS = os.path.dirname(os.path.realpath(__file__))


def _load(name, file):
    loader = importlib.machinery.SourceFileLoader(name, os.path.join(SCRIPTS, file))
    module = importlib.util.module_from_spec(importlib.util.spec_from_loader(name, loader))
    loader.exec_module(module)
    return module


gc = _load("target_gc", "target-gc.py")
check = _load("check_compile_shape", "check-compile-shape.py")


def target_name(path):
    """The target cargo names the unit by; scripts/target-gc.py's `Unit.target` can pick the `output-` diagnostics file."""
    found = glob.glob(os.path.join(path, "fingerprint", "*.json"))
    assert len(found) == 1, "%s holds %d fingerprint json files, want 1" % (path, len(found))
    return os.path.basename(found[0])[:-len(".json")]


def fingerprint(path):
    """The unit's fingerprint json; `features` parsed to a list, `local` reduced to the env vars a build script reads."""
    with open(os.path.join(path, "fingerprint", target_name(path) + ".json")) as f:
        fp = json.load(f)
    fp["features"] = json.loads(fp["features"]) if fp["features"] else []
    fp["local"] = sorted((e["RerunIfEnvChanged"]["var"], e["RerunIfEnvChanged"]["val"])
                         for e in fp["local"] if "RerunIfEnvChanged" in e)
    return fp


def mode(unit):
    if target_name(unit.path).startswith("run-build-script"):
        return "run"
    out = os.path.join(unit.path, "out")
    names = os.listdir(out) if os.path.isdir(out) else []
    built = any(n.endswith((".rlib", ".dylib", ".so", ".a")) or (os.access(os.path.join(out, n), os.X_OK)
                and os.path.isfile(os.path.join(out, n))) for n in names)
    return "build" if built or not any(n.endswith(".rmeta") for n in names) else "check"


def hash_index(units):
    """The dir of every unit by the fingerprint hash its dependents name in their `deps`."""
    index = {}
    for u in units:
        for p in glob.glob(os.path.join(u.path, "fingerprint", "*.json")):
            if os.path.isfile(p[:-len(".json")]):
                with open(p[:-len(".json")]) as f:
                    index[int.from_bytes(bytes.fromhex(f.read().strip()), "little")] = u.path
    return index


def differ(a, b):
    fa, fb = fingerprint(a), fingerprint(b)
    return fa, fb, sorted(k for k in set(fa) | set(fb) if fa.get(k) != fb.get(k))


SAME_UNIT = "no fingerprint field differs (a workspace member under cargo clippy vs cargo check)"


def root_cause(a, b, index, seen=None):
    """`crate: field ...` of the first pair down the deps chain of a and b that differs in more than deps."""
    seen = seen or set()
    fa, fb, fields = differ(a, b)
    crate = os.path.basename(os.path.dirname(a))
    if not fields:
        return "%s: %s" % (crate, SAME_UNIT)
    if fields != ["deps"]:
        detail = []
        if "features" in fields:
            detail.append("features %s vs %s" % (sorted(set(fa["features"]) - set(fb["features"])),
                                                  sorted(set(fb["features"]) - set(fa["features"]))))
        if "local" in fields:
            ea, eb = dict(fa["local"]), dict(fb["local"])
            env = sorted(v for v in set(ea) | set(eb) if ea.get(v, ()) != eb.get(v, ()))
            detail.append("build script env %s vs %s" % ({v: ea[v] for v in env if v in ea}, {v: eb[v] for v in env if v in eb})
                          if env else "build script env read in another order or twice")
        detail += [f for f in fields if f not in ("features", "local", "deps")]
        return "%s: %s" % (crate, ", ".join(detail))
    da, db = {d[1]: d[3] for d in fa["deps"]}, {d[1]: d[3] for d in fb["deps"]}
    names = [n for n in sorted(set(da) | set(db)) if da.get(n) != db.get(n)]
    for name in names:
        ha, hb = da.get(name), db.get(name)
        if ha is None or hb is None:
            return "%s: dep %s only in one dir" % (crate, name)
    for name in names:
        ha, hb = da[name], db[name]
        if ha in index and hb in index and (ha, hb) not in seen:
            seen.add((ha, hb))
            return "%s <- %s" % (crate, root_cause(index[ha], index[hb], index, seen))
    if names:
        return "%s: dep %s was rebuilt in place after this dir was built" % (crate, names[0])
    return "%s: deps differ in order only" % crate


def pair_key(profile, crate, tgt, a, b, index):
    """The pair without its hash dir names, in either order of a and b."""
    return min("%s/%s %s: %s" % (profile, crate, tgt, root_cause(x, y, index)) for x, y in ((a, b), (b, a)))


def load_baseline(path):
    with open(path) as f:
        return collections.Counter(line.rstrip("\n") for line in f if line.strip() and not line.startswith("#"))


def take(baseline, seen, key):
    """True, and one baseline line of key used up, while the baseline holds a line of key not yet used."""
    if seen[key] >= baseline[key]:
        return False
    seen[key] += 1
    return True


def replay(target, calls):
    with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as f:
        f.write("".join(json.dumps(c) + "\n" for c in calls))
    try:
        return gc.in_use(target, f.name)
    finally:
        os.unlink(f.name)


COMPILE_MODE = {"clippy": "check", "check": "check", "test": "build", "nextest": "build", "build": "build"}


def classes(target, record):
    """{realpath: 'canon' | 'exception'} of the dirs the recorded calls use, and {realpath: {(replay, compile mode)}}
    of the replays of CANON calls that use each dir. Calls with one replay argv and env share one replay."""
    canon, excepted = [], []
    for call in check.load(record):
        sub, args = check.subcommand(call["argv"])
        pre = call["argv"][:len(call["argv"]) - len(args) - 1]
        if sub == "nextest" and args:
            args = args[1:]
        (excepted if check.exception(sub, check.reduce(sub, pre + args), args) else canon).append(call)
    out = {p: "exception" for p in replay(target, excepted)} if excepted else {}
    groups = {}
    for call in canon:
        groups.setdefault(json.dumps([gc.replay_argv(check, call["argv"]), sorted(call["env"].items())]), []).append(call)
    users = collections.defaultdict(set)
    for i, group in enumerate(groups.values()):
        dirs = replay(target, group)
        if dirs:
            compile_mode = COMPILE_MODE[check.subcommand(group[0]["argv"])[0]]
            print("replay %d, %s mode, uses %d dir(s): cargo %s" % (i, compile_mode, len(dirs), " ".join(group[0]["argv"])))
        for p in dirs:
            out[p] = "canon"
            users[p].add((i, compile_mode))
    return out, users


def verdict(a, b, users, index):
    """'one call', 'check vs build' (see the module doc) or 'second shape' for two used dirs of one profile hash."""
    ua, ub = users.get(os.path.realpath(a), frozenset()), users.get(os.path.realpath(b), frozenset())
    if {i for i, _ in ua} & {i for i, _ in ub}:
        return "one call"
    if {frozenset(m for _, m in ua), frozenset(m for _, m in ub)} == {frozenset({"check"}), frozenset({"build"})} \
            and root_cause(a, b, index).endswith(SAME_UNIT):
        return "check vs build"
    return "second shape"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("target")
    parser.add_argument("--mark", required=True)
    parser.add_argument("--record")
    parser.add_argument("--baseline", default=os.path.join(SCRIPTS, "compile-shape-baseline.txt"))
    args = parser.parse_args()
    units = gc.collect_units(args.target)
    cutoff = os.stat(args.mark).st_mtime
    since = {os.path.realpath(u.path) for u in units if gc.used_since(u.path, cutoff)}
    cls, users = classes(args.target, args.record) if args.record else ({}, {})
    used = [u for u in units if os.path.realpath(u.path) in since | set(cls)]
    print("used: %d of %d hash dirs (%d built at or after %s%s)" % (
        len(used), len(units), len(since), args.mark, ", %d used by a replay of %s" % (len(cls), args.record) if cls else ""))
    index = hash_index(units)
    packages = collections.defaultdict(list)
    for u in used:
        packages[((u.profile, u.crate, target_name(u.path)), fingerprint(u.path)["path"])].append(u)
    dist = collections.Counter(len(v) for v in packages.values())
    print("packages: %d; used dirs per package: %s" % (
        len(packages), ", ".join("%d dir(s): %d" % (n, c) for n, c in sorted(dist.items()))))
    baseline, seen = load_baseline(args.baseline), collections.Counter()
    fails = []
    for ((profile, crate, tgt), _path), members in sorted(packages.items(), key=lambda kv: kv[0][0]):
        if len(members) == 1:
            continue
        kinds = collections.defaultdict(list)
        print("%s/%s %s:" % (profile, crate, tgt))
        for u in sorted(members, key=lambda u: u.hash):
            fp = fingerprint(u.path)
            c = cls.get(os.path.realpath(u.path), "unrecorded" if cls else "since mark")
            print("  %s %s profile %d, %d feature(s), %s%s" % (u.hash, mode(u), fp["profile"], len(fp["features"]), c, "".join(
                ", replay %d (%s)" % r for r in sorted(users.get(os.path.realpath(u.path), ())))))
            if c == "unrecorded":
                fails.append("UNRECORDED: %s/%s %s %s is used, and no recorded call uses it" % (profile, crate, tgt, u.hash))
            if c != "exception":
                kinds[fp["profile"]].append(u)
        for prof, us in kinds.items():
            for i, a in enumerate(us):
                for b in us[i + 1:]:
                    v = verdict(a.path, b.path, users, index)
                    if v != "second shape":
                        print("  %s and %s: %s" % (a.hash, b.hash, v))
                        continue
                    key = pair_key(profile, crate, tgt, a.path, b.path, index)
                    if take(baseline, seen, key):
                        print("  %s and %s: baseline" % (a.hash, b.hash))
                        continue
                    fails.append("SECOND SHAPE: %s/%s %s, dirs of profile %d: %s and %s; cause %s" % (
                        profile, crate, tgt, prof, a.hash, b.hash, root_cause(a.path, b.path, index)))
    if args.record:
        fails += check.lint_fails(check.load(args.record), check.clippy_lints())
    print("baseline %s: %d pair(s) seen, %d not seen" % (
        args.baseline, sum(seen.values()), sum(baseline.values()) - sum(seen.values())))
    print("\n".join(fails))
    print("count-hash-folders: %d package(s) with more than one used dir, %d failure(s)" % (
        sum(len(v) > 1 for v in packages.values()), len(fails)))
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
