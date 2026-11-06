#!/usr/bin/python3
"""Fail when the recorded cargo calls build more than one compile shape.

    scripts/check-compile-shape.py <record.jsonl>

The record is what scripts/cargo-shape/cargo wrote. Exit 0 iff every compiling call that no EXCEPTIONS entry names
1. has build options (BUILD_OPTIONS, reduced from its argv) equal to the justfile's CANON, and
2. sees the same value of every recorded build-script variable as every other such call, with HOLON_BUILD_TIME unset.
A cargo call with other build options or another value of such a variable makes cargo mint other unit hashes or
rebuild a unit in place, so the gate compiles twice. Exit 1 names each offending call; an empty or missing record
and an unknown cargo subcommand are failures too.
"""
import json
import os
import shlex
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))

# The options that pick the units and their features. Every other option only narrows what runs or how it is shown.
BUILD_OPTIONS = {"--workspace": 0, "--all": 0, "-p": 1, "--package": 1, "--exclude": 1, "--features": 1, "-F": 1,
                 "--all-features": 0, "--no-default-features": 0, "--release": 0, "--target": 1,
                 "--manifest-path": 1, "--config": 1}
# cargo's own `--profile`; nextest's `--profile` names a nextest profile and its `--cargo-profile` is cargo's.
CARGO_PROFILE = {"nextest": "--cargo-profile"}
COMPILING = {"test", "check", "clippy", "build", "nextest"}
NEXTEST_COMPILING = {"run", "r", "list", "archive"}
NON_COMPILING = {"fmt", "metadata", "deny", "tree", "update", "machete", "locate-project", "pkgid", "version",
                 "--version", "-V", "-vV", "help", "--help"}
GLOBAL_WITH_VALUE = {"-Z", "-C", "--color", "--config"}
# Off-shape by necessity: (name, reason, predicate over the subcommand, its reduced build options and its args).
# The justfile's CANON comment points here.
EXCEPTIONS = [
    ("app and tool launches", "`cargo run` rejects --workspace; install, watch, mutants, ndk, llvm-cov and crap "
     "build into their own dirs or are no gate step",
     lambda sub, red, args: sub in {"run", "install", "binstall", "watch", "mutants", "ndk", "llvm-cov", "crap"}),
    ("another target triple", "a --target triple owns its own unit dirs, so it shares no hash with the host shape",
     lambda sub, red, args: "--target" in red),
    ("out-of-workspace manifests", "holon-worker, dioxus-web and the guests are no workspace members",
     lambda sub, red, args: "--manifest-path" in red and red["--manifest-path"] != ["Cargo.toml"]),
    ("release shape", "check-release-shape builds the feature set without any dev-dependency",
     lambda sub, red, args: sub == "check" and "--lib" in args and "--bins" in args and "--features" not in red),
    ("capability certificate", "scripts/capability-cert-native.sh needs holon/test-helpers",
     lambda sub, red, args: "holon/test-helpers" in ",".join(red.get("--features", []))),
]
# nextest runs prebuilt binaries with these; it builds nothing.
NEXTEST_REUSE = {"--binaries-metadata", "--archive-file", "--cargo-metadata"}


def subcommand(argv):
    """(subcommand, the args after it); a toolchain `+x` and global options before it are skipped."""
    i = 0
    while i < len(argv):
        a = argv[i]
        if a.startswith("+"):
            i += 1
        elif a in GLOBAL_WITH_VALUE:
            i += 2
        elif a.startswith("-") and a not in NON_COMPILING:
            i += 1
        else:
            return a, argv[i + 1:]
    return None, []


def reduce(sub, args):
    """The build options of a call as {option: [values]}; features are a sorted set, `--all` reads as --workspace."""
    profile = CARGO_PROFILE.get(sub, "--profile")
    out = {}
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--":
            break
        name, eq, val = a.partition("=")
        if name.startswith("-p") and len(name) > 2 and not name.startswith("--"):
            name, val, eq = "-p", name[2:], "="
        if name == profile:
            name = "--cargo-profile"
            arity = 1
        else:
            arity = BUILD_OPTIONS.get(name)
        if arity is None:
            i += 1
            continue
        if arity == 0:
            out.setdefault(name, []).append(True)
            i += 1
            continue
        if not eq:
            if i + 1 >= len(args):
                raise SystemExit("check-compile-shape: %s without a value in %s" % (name, args))
            val = args[i + 1]
            i += 1
        i += 1
        out.setdefault(name, []).append(val)
    canon = {}
    for name, vals in out.items():
        name = {"--all": "--workspace", "--package": "-p", "-F": "--features"}.get(name, name)
        canon.setdefault(name, []).extend(vals)
    if "--features" in canon:
        canon["--features"] = [",".join(sorted({f for v in canon["--features"] for f in v.replace(" ", ",").split(",") if f}))]
    if "--workspace" in canon:
        canon["--workspace"] = [True]
    return canon


def canon():
    out = subprocess.run(["just", "--justfile", os.path.join(REPO, "justfile"), "--evaluate", "CANON"],
                         capture_output=True, text=True)
    if out.returncode != 0 or not out.stdout.strip():
        raise SystemExit("check-compile-shape: just --evaluate CANON failed: %s" % out.stderr.strip())
    return reduce("check", shlex.split(out.stdout))


def exception(sub, red, args):
    for name, _reason, pred in EXCEPTIONS:
        if pred(sub, red, args):
            return name
    return None


def judge(calls, want):
    """The failure lines for the recorded calls; empty iff the record is one shape."""
    if not calls:
        return ["no cargo calls recorded"]
    fails, judged = [], []
    for call in calls:
        argv = call["argv"]
        sub, args = subcommand(argv)
        shown = "cargo " + shlex.join(argv)
        if sub in NON_COMPILING or sub is None:
            continue
        if sub == "nextest":
            nsub = args[0] if args else None
            if nsub not in NEXTEST_COMPILING:
                if nsub in {"show-config", "self", "help", "--help", "--version", "-V"}:
                    continue
                fails.append("UNKNOWN SUBCOMMAND: cargo nextest %s, in %s" % (nsub, shown))
                continue
            args = args[1:]
            if NEXTEST_REUSE & {a.partition("=")[0] for a in args}:
                continue
        red = reduce(sub, args)
        if exception(sub, red, args):
            continue
        if sub not in COMPILING:
            fails.append("UNKNOWN SUBCOMMAND: cargo %s may compile; list it in COMPILING, NON_COMPILING or "
                         "EXCEPTIONS of %s, in %s" % (sub, os.path.relpath(__file__, REPO), shown))
            continue
        red.pop("--manifest-path", None)
        if red != want:
            fails.append("OFF SHAPE: %s builds %s, not CANON %s" % (shown, fmt(red), fmt(want)))
        judged.append((shown, call["env"]))
    for shown, env in judged:
        if env.get("HOLON_BUILD_TIME") is not None:
            fails.append("BUILD TIME SET: %s sees HOLON_BUILD_TIME; test builds leave it unset" % shown)
    names = sorted({n for _s, env in judged for n in env})
    for name in names:
        seen = {}
        for shown, env in judged:
            if name in env:
                seen.setdefault(env[name], []).append(shown)
        if len(seen) > 1:
            fails.append("ENV DIFFERS: %s is %s" % (name, "; ".join(
                "%s in %d call(s), first %s" % ("unset" if v is None else repr(v), len(c), c[0])
                for v, c in seen.items())))
    return fails


def fmt(red):
    return " ".join(k if v == [True] else " ".join("%s %s" % (k, x) for x in v) for k, v in sorted(red.items())) or "(none)"


def load(path):
    try:
        with open(path) as f:
            return [json.loads(line) for line in f if line.strip()]
    except FileNotFoundError:
        return []


def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    calls = load(sys.argv[1])
    fails = judge(calls, canon())
    for line in fails:
        print(line)
    print("check-compile-shape: %d call(s) in %s, %d failure(s)" % (len(calls), sys.argv[1], len(fails)))
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
