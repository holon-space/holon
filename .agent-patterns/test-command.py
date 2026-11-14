#!/usr/bin/env python3
"""test-command.py features <unit>... | run [--scope <file>] <build workspace> <unit>... : the hook's `test-command` answer.

`features` prints `--features <list>` (or nothing): the features of scripts/select-tests.py's canonical list that belong
to the given units; cargo refuses a `<pkg>/<feature>` whose package is not selected.
`run` prints the nextest run, one word per line. With `--scope`, it selects the test targets the listed files concern:
a file under <unit>/tests/<name>[.rs|/...] concerns the test target <name>, or the targets whose root declares
`mod <name>;`; any other file of a unit concerns all of the unit's tests. Files outside the units concern nothing.
Runs in the workspace root.
"""
import importlib.util
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path, PurePosixPath

ROOT = Path.cwd()


def canon_features():
    spec = importlib.util.spec_from_file_location("select_tests", ROOT / "scripts/select-tests.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.canon_features(ROOT)


def feature_args(units):
    feats = [f for f in canon_features() if f.split("/")[0] in units]
    return ["--features", ",".join(feats)] if feats else []


def packages(units):
    meta = json.loads(subprocess.run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--offline"],
                                     stdout=subprocess.PIPE, check=True, text=True).stdout)
    pkgs = {p["name"]: p for p in meta["packages"]}
    missing = [u for u in units if u not in pkgs]
    if missing:
        sys.exit(f"no package {', '.join(missing)} in {ROOT}")
    return {u: pkgs[u] for u in units}


def concerned_targets(pkg, rel):
    """The test target names a file at `rel` (relative to the package dir) concerns; None means the whole package."""
    if rel.parts[0] != "tests" or len(rel.parts) < 2:
        return None
    name = PurePosixPath(rel.parts[1]).stem
    tests = {t["name"]: Path(t["src_path"]) for t in pkg["targets"] if t["kind"] == ["test"]}
    if name in tests:
        return {name}
    declares = re.compile(rf"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+{re.escape(name)}\s*;", re.M)
    owners = {t for t, src in tests.items() if declares.search(src.read_text())}
    if owners:
        return owners
    manifest = tomllib.loads(Path(pkg["manifest_path"]).read_text())
    if manifest["package"].get("autotests", True):
        # A new test file of the tester: cargo discovers it as its own target in the tester's workspace.
        return {name}
    if "main" in tests:
        # The kit's register-tests adds the new file's `mod` line to the single test binary tests/main.rs.
        return {"main"}
    sys.exit(f"{pkg['name']}/{rel}: no test target of the package concerns it and the package has no tests/main.rs")


def scope_filter(units, scope_file):
    pkgs = packages(units)
    dirs = {u: PurePosixPath(Path(p["manifest_path"]).parent.relative_to(ROOT)) for u, p in pkgs.items()}
    whole, targets = set(), {}
    for line in Path(scope_file).read_text().splitlines():
        if not line:
            continue
        path = PurePosixPath(line)
        owner = max((u for u, d in dirs.items() if path.is_relative_to(d)), key=lambda u: len(dirs[u].parts),
                    default=None)
        if owner is None:
            continue
        names = concerned_targets(pkgs[owner], path.relative_to(dirs[owner]))
        if names is None:
            whole.add(owner)
        else:
            targets.setdefault(owner, set()).update(names)
    for u in whole:
        targets.pop(u, None)
    if not whole and not targets:
        sys.exit(f"{scope_file}: no listed file lies in the units {', '.join(units)}: no test concerns them")
    terms = [f"package(={u})" for u in sorted(whole)]
    terms += [f"binary_id(={u}::{t})" for u in sorted(targets) for t in sorted(targets[u])]
    selected = sorted(whole | set(targets))
    narrow = [] if whole else [a for t in sorted(set().union(*targets.values())) for a in ("--test", t)]
    return selected, narrow + ["-E", " | ".join(terms)]


def main(argv):
    op, args = argv[0], argv[1:]
    if op == "features":
        print(" ".join(feature_args(args)))
        return
    assert op == "run", f"unknown op {op}"
    scope = None
    if args[0] == "--scope":
        scope, args = args[1], args[2:]
    build_ws, units = args[0], args[1:]
    assert units, "test-command names no unit"
    selected, filt = scope_filter(units, scope) if scope else (units, [])
    words = [f"CARGO_TARGET_DIR={build_ws}/target", "cargo", "nextest", "run"]
    words += [w for u in selected for w in ("-p", u)] + feature_args(selected) + filt + ["--no-fail-fast"]
    print("\n".join(words))


main(sys.argv[1:])
