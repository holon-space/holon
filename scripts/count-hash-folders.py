#!/usr/bin/python3
"""Count the hash dirs one gate used per unit, and fail on a unit built in a second compile shape.

    scripts/count-hash-folders.py <target dir> --mark <file> [--record <cargo-calls.jsonl>]

A used dir is one built at or after the mark; with a record, also every dir a replay of the recorded
calls uses (scripts/target-gc.py `in_use`), so reused fresh units count too. Inside one package, one dir per cargo
profile hash (dev, test, build-override; check and build mode hash differently) is the single shape. Exit 1 on a
second dir of the same hash that a CANON call (without a record: any call) uses, naming the first crate down the
deps chain that differs in more than its deps, and on a used dir no recorded call uses. Dirs only the EXCEPTIONS of
scripts/check-compile-shape.py use are listed, not failed.
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


def root_cause(a, b, index, seen=None):
    """`crate: field ...` of the first pair down the deps chain of a and b that differs in more than deps."""
    seen = seen or set()
    fa, fb, fields = differ(a, b)
    crate = os.path.basename(os.path.dirname(a))
    if not fields:
        return "%s: no fingerprint field differs (a workspace member under cargo clippy vs cargo check)" % crate
    if fields != ["deps"]:
        detail = []
        if "features" in fields:
            detail.append("features %s vs %s" % (sorted(set(fa["features"]) - set(fb["features"])),
                                                  sorted(set(fb["features"]) - set(fa["features"]))))
        if "local" in fields:
            detail.append("build script env %s vs %s" % (dict(fa["local"]), dict(fb["local"])))
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


def replay(target, calls):
    with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as f:
        f.write("".join(json.dumps(c) + "\n" for c in calls))
    try:
        return gc.in_use(target, f.name)
    finally:
        os.unlink(f.name)


def classes(target, record):
    """{realpath: 'canon' | 'exception'} of the dirs the recorded calls use."""
    canon, excepted = [], []
    for call in check.load(record):
        sub, args = check.subcommand(call["argv"])
        pre = call["argv"][:len(call["argv"]) - len(args) - 1]
        if sub == "nextest" and args:
            args = args[1:]
        (excepted if check.exception(sub, check.reduce(sub, pre + args), args) else canon).append(call)
    out = {p: "exception" for p in replay(target, excepted)} if excepted else {}
    out.update({p: "canon" for p in replay(target, canon)})
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("target")
    parser.add_argument("--mark", required=True)
    parser.add_argument("--record")
    args = parser.parse_args()
    units = gc.collect_units(args.target)
    cutoff = os.stat(args.mark).st_mtime
    since = {os.path.realpath(u.path) for u in units if gc.used_since(u.path, cutoff)}
    cls = classes(args.target, args.record) if args.record else {}
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
    fails = []
    for ((profile, crate, tgt), _path), members in sorted(packages.items(), key=lambda kv: kv[0][0]):
        if len(members) == 1:
            continue
        kinds = collections.defaultdict(list)
        print("%s/%s %s:" % (profile, crate, tgt))
        for u in sorted(members, key=lambda u: u.hash):
            fp = fingerprint(u.path)
            c = cls.get(os.path.realpath(u.path), "unrecorded" if cls else "since mark")
            print("  %s %s profile %d, %d feature(s), %s" % (u.hash, mode(u), fp["profile"], len(fp["features"]), c))
            if c == "unrecorded":
                fails.append("UNRECORDED: %s/%s %s %s is used, and no recorded call uses it" % (profile, crate, tgt, u.hash))
            if c != "exception":
                kinds[fp["profile"]].append(u)
        for prof, us in kinds.items():
            for other in us[1:]:
                fails.append("SECOND SHAPE: %s/%s %s, dirs of profile %d: %s and %s; cause %s" % (
                    profile, crate, tgt, prof, us[0].hash, other.hash, root_cause(us[0].path, other.path, index)))
    print("\n".join(fails))
    print("count-hash-folders: %d package(s) with more than one used dir, %d failure(s)" % (
        sum(len(v) > 1 for v in packages.values()), len(fails)))
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
