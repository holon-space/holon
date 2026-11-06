#!/usr/bin/python3
"""Property tests for scripts/check-compile-shape.py.

    scripts/test-check-compile-shape.py [cases]              the check's verdicts; no cargo
    scripts/test-check-compile-shape.py --unit-graph [cases]  also the narrowing rule the check trusts, against
                                                              cargo's unit graph (resolves, builds nothing)

SEED=<n> replays a run. CHECK=<path> runs the properties against another copy of the check.
Properties:
- a call of CANON, spelt any way, plus any narrowing (`--test`, `--no-run`, nextest `--profile`, `-E`, test filters) passes;
- one off-shape build option, or one shape-env value that differs from the other calls, or HOLON_BUILD_TIME set,
  fails the check;
- with --unit-graph: the units of a narrowed call are a subset of the units of `cargo test CANON --no-run`.
"""
import functools
import glob
import importlib.machinery
import importlib.util
import json
import os
import random
import shlex
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
CHECK = os.environ.get("CHECK", os.path.join(REPO, "scripts", "check-compile-shape.py"))
ENV = {"RUST_FONTCONFIG_DLOPEN": "on", "HOLON_BUILD_SHA": "0123456789ab", "HOLON_BUILD_TIME": None,
       "RUSTFLAGS": None}
# Written out here, not read from the check, so a check that forgets one is caught.
OFF_SHAPE = [["-p", "holon-org-format"], ["--package=holon-api"], ["-pholon-core"],
             ["--features", "holon-gpui/chrome-trace"], ["-F", "holon-gpui/chrome-trace"],
             ["--no-default-features"], ["--all-features"], ["--release"], ["--exclude", "holon-gpui"],
             ["--config", "profile.dev.opt-level=1"], ["-r"], ["-Fholon-gpui/chrome-trace"]]
STATIC_TESTS = ["integration", "deep_authored_text_never_aborts"]
# Off-shape calls the check lets pass by its EXCEPTIONS list; a check that drops one refuses its case.
EXCEPTED = [["run", "-p", "holon-gpui", "--features", "chrome-trace"],
            ["check", "--target", "wasm32-unknown-unknown", "-p", "holon-web"],
            ["build", "--manifest-path", "frontends/holon-worker/Cargo.toml", "--release"],
            ["check", "--workspace", "--lib", "--bins"],
            ["test", "--workspace", "--features", "holon/test-helpers", "--test", "capability_cert"]]


def load_check():
    loader = importlib.machinery.SourceFileLoader("check_compile_shape", CHECK)
    spec = importlib.util.spec_from_loader(loader.name, loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


@functools.lru_cache(None)
def canon_args():
    return tuple(shlex.split(subprocess.run(["just", "--justfile", os.path.join(REPO, "justfile"), "--evaluate", "CANON"],
                                      capture_output=True, text=True, check=True).stdout))


def test_files():
    """Test target names from the tree, without cargo."""
    return sorted({os.path.basename(p)[:-3] for p in glob.glob(os.path.join(REPO, "crates", "*", "tests", "*.rs"))})


def test_names():
    meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=REPO,
                                     capture_output=True, text=True, check=True).stdout)
    return sorted({t["name"] for p in meta["packages"] for t in p["targets"] if "test" in t["kind"]})


def narrowing(rng, tests):
    """(subcommand words, args): CANON with a random subset of the narrowing vocabulary."""
    nextest = rng.random() < 0.5
    args = list(canon_args())
    words = ["nextest", "run"] if nextest else ["test"]
    # Spellings of the same build options: `--all`, feature order, `--features=`.
    if rng.random() < 0.3:
        args[args.index("--workspace")] = "--all"
    i = args.index("--features")
    feats = args[i + 1].split(",")
    rng.shuffle(feats)
    args[i:i + 2] = ["--features=" + ",".join(feats)] if rng.random() < 0.3 else ["--features", ",".join(feats)]
    picked = rng.sample(tests, rng.choice([0, 1, 1, 2]))
    for t in picked:
        args += ["--test", t]
    if rng.random() < 0.5:
        args.append("--no-run")
    if nextest and rng.random() < 0.5:
        args += ["--profile", rng.choice(["default", "load-tolerant", "timing"])]
    if nextest and rng.random() < 0.5:
        args += ["-E", "test(/%s/)" % rng.choice(["org_suite", "views", "a b"])]
    if not nextest and rng.random() < 0.5:
        args += ["--", "--ignored", "--test-threads=1"]
    return words, args


def breaks(args):
    """Indices where a token can go without splitting an option from its value or landing after `--`."""
    end = args.index("--") if "--" in args else len(args)
    takes_value = {"--test", "--profile", "-E", "--features", "-p", "--package"}
    return [i for i in range(end + 1) if i == 0 or args[i - 1] not in takes_value]


def without_one_option(rng):
    """CANON with one of its build options dropped: --workspace, or the feature list."""
    args = list(canon_args())
    i = rng.choice([i for i, a in enumerate(args) if a.startswith("--")])
    return args[:i] + args[i + 1 + (args[i] == "--features"):]


def call(words, args, **env):
    return {"argv": [*words, *args], "env": {**ENV, **env}}


def verdict(check, want, calls):
    return check.judge(calls, want)


def check_properties(check, rng, cases, tests):
    want = check.canon()
    canon = canon_args()
    examples = [(["test"], [*canon, "--test", t, "--no-run"]) for t in STATIC_TESTS]
    for i in range(len(examples) + cases):
        words, args = examples[i] if i < len(examples) else narrowing(rng, tests)
        base = call(words, args)
        fails = verdict(check, want, [base, call(["clippy"], [*canon, "--all-targets", "--", "-D", "warnings"])])
        assert not fails, "narrowed call refused: cargo %s\n%s" % (shlex.join(base["argv"]), fails)

        token = rng.choice(OFF_SHAPE)
        cut = rng.choice(breaks(args))
        off = call(words, args[:cut] + token + args[cut:])
        assert verdict(check, want, [base, off]), "off-shape token %s passed in cargo %s" % (token, shlex.join(off["argv"]))
        short = call(words, without_one_option(rng))
        assert verdict(check, want, [base, short]), "cargo %s passed" % shlex.join(short["argv"])

        name = rng.choice(sorted(ENV))
        other = rng.choice(["off", "", None, "1"] if ENV[name] is not None else ["on", ""])
        if other == ENV[name]:
            other = "x"
        assert verdict(check, want, [base, call(words, args, **{name: other})]), \
            "%s=%r beside %r passed" % (name, other, ENV[name])
    for argv in EXCEPTED:
        fails = verdict(check, want, [call(argv[:1], [*canon, "--no-run"]), call(argv[:1], argv[1:])])
        assert not fails, "excepted call refused: cargo %s\n%s" % (shlex.join(argv), fails)
    glob = call(["--config", "profile.dev.debug=0", "test"], [*canon, "--no-run"])
    assert verdict(check, want, [call(["test"], [*canon, "--no-run"]), glob]), "global --config passed: cargo %s" % shlex.join(glob["argv"])
    assert verdict(check, want, [call(["test"], canon, HOLON_BUILD_TIME="1")]), "HOLON_BUILD_TIME set passed"
    assert verdict(check, want, []) == ["no cargo calls recorded"]


def check_exclusions():
    """The planted records the check must refuse, through its command line."""
    canon = canon_args()
    planted = {
        "a recipe with its own feature": [call(["test"], [*canon, "--features", "holon-gpui/chrome-trace"])],
        "a call without RUST_FONTCONFIG_DLOPEN": [call(["test"], canon),
                                                  call(["nextest", "run"], canon, RUST_FONTCONFIG_DLOPEN=None)],
        "an empty record": [],
        "a missing record": None,
    }
    with tempfile.TemporaryDirectory() as tmp:
        for what, calls in planted.items():
            path = os.path.join(tmp, "calls.jsonl")
            if os.path.exists(path):
                os.remove(path)
            if calls is not None:
                with open(path, "w") as f:
                    f.writelines(json.dumps(c) + "\n" for c in calls)
            r = subprocess.run([sys.executable, CHECK, path], capture_output=True, text=True)
            print("%s: exit %d, %s" % (what, r.returncode, r.stdout.strip().splitlines()[0]))
            assert r.returncode == 1, "%s passed the check:\n%s%s" % (what, r.stdout, r.stderr)


INTERNED = {}
NAMES = {}


def unit_ids(words, args):
    """The identities of the units `cargo <words> <args>` would build; nextest builds what `cargo test` does.
    An identity is an int shared by all calls: a nested tuple would rehash its whole dependency tree per lookup."""
    build = []
    skip = {"--profile": 1, "-E": 1, "--no-run": 0}
    i = 0
    while i < len(args) and args[i] != "--":
        if args[i] in skip:
            i += 1 + skip[args[i]]
            continue
        build.append(args[i])
        i += 1
    out = subprocess.run(["cargo", "test", *build, "--no-run", "--unit-graph", "-Z", "unstable-options"], cwd=REPO,
                         capture_output=True, text=True)
    assert out.returncode == 0, "unit graph of cargo test %s failed:\n%s" % (shlex.join(build), out.stderr[-2000:])
    units = json.loads(out.stdout)["units"]
    memo = {}

    def ident(i):
        if i not in memo:
            u = dict(units[i])
            deps = u.pop("dependencies")
            # The whole unit: cargo varies a unit's profile (debuginfo of a build dependency) with the graph around it.
            key = (json.dumps(u, sort_keys=True), tuple(sorted(ident(d["index"]) for d in deps)))
            memo[i] = INTERNED.setdefault(key, len(INTERNED))
            NAMES[memo[i]] = "%s %s %s" % (u["pkg_id"].rsplit("#", 1)[-1], u["mode"], u["target"]["name"])
        return memo[i]

    return {ident(i) for i in range(len(units))}


def check_unit_graph(rng, cases, tests):
    canon = canon_args()
    whole = unit_ids(["test"], canon)
    examples = [(["test"], [*canon, "--test", t]) for t in STATIC_TESTS]
    for i in range(len(examples) + cases):
        words, args = examples[i] if i < len(examples) else narrowing(rng, tests)
        extra = unit_ids(words, args) - whole
        print("unit graph: cargo %s: %d unit(s) outside CANON" % (shlex.join([*words, *args]), len(extra)))
        assert not extra, [NAMES[x] for x in sorted(extra)[:5]]


def main():
    argv = sys.argv[1:]
    graph = argv[:1] == ["--unit-graph"]
    cases = int(argv[1 if graph else 0]) if len(argv) > (1 if graph else 0) else (6 if graph else 200)
    seed = int(os.environ.get("SEED", random.randrange(1 << 30)))
    print("SEED=%d" % seed)
    rng = random.Random(seed)
    tests = test_names() if graph else test_files()
    check_properties(load_check(), rng, cases if not graph else 200, tests)
    check_exclusions()
    if graph:
        check_unit_graph(rng, cases, tests)
    print("ok")


if __name__ == "__main__":
    main()
