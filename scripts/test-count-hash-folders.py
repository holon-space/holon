#!/usr/bin/env python3
"""Self-check for scripts/count-hash-folders.py on generated fake target dirs.

Run: `python3 scripts/test-count-hash-folders.py`. Exits non-zero on the first violation.
COUNT_HASH_FOLDERS=<path> runs the checks against another copy of the script. SEED=<n> replays the generated cases;
EXAMPLE=<i> runs the fixed example i only.
"""
import collections
import importlib.machinery
import importlib.util
import json
import os
import random
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCRIPT = Path(os.environ.get("COUNT_HASH_FOLDERS", REPO / "scripts" / "count-hash-folders.py"))
CHECK, BUILD = 1001, 1002
BEFORE_MARK, AFTER_MARK = 1_000_000_000, 2_000_000_000

# A twin that differs in its first differing crate in one of these fields; "wrapper" differs in no field.
DIFFERENCES = ("features", "rustflags", "env", "wrapper")


def unit(target, crate, rng, fp_value, deps, built, *, features=("default",), rustflags=(), env=None, profile=CHECK):
    path = target / "debug" / "build" / crate / ("%016x" % rng.getrandbits(64))
    (path / "fingerprint").mkdir(parents=True)
    (path / "out").mkdir()
    name = "lib-" + crate.replace("-", "_")
    fp = {"rustc": 1, "features": json.dumps(list(features)), "declared_features": "", "target": 7, "profile": profile,
          "path": int(crate[1:]), "deps": [[1, d, False, v] for d, v in deps], "rustflags": list(rustflags),
          "local": [{"CheckDepInfo": {"dep_info": str(path)}}] + (
              [{"RerunIfEnvChanged": {"var": "SHA", "val": env}}] if env is not None else []),
          "config": 0, "compile_kind": 0}
    (path / "fingerprint" / (name + ".json")).write_text(json.dumps(fp))
    (path / "fingerprint" / name).write_text(fp_value.to_bytes(8, "little").hex())
    (path / "out" / ("lib%s-x.%s" % (crate, "rmeta" if profile == CHECK else "rlib"))).write_text("")
    for p in [*path.rglob("*"), path]:
        os.utime(p, (built, built))
    return path


# Answers a replay with the artifacts the case gives the recorded call it replays.
FAKE_CARGO = """#!/usr/bin/python3
import json, os, sys
call = " ".join(a for a in sys.argv[1:] if a not in ("--no-run", "--message-format=json"))
sys.stdout.write(json.load(open(os.environ["FAKE_CARGO_ANSWERS"]))[call])
"""
TEST_CALL = ["test", "--workspace"]
OTHER_TEST_CALL = ["test", "--workspace", "--test", "t1"]
CLIPPY_LINTS = shlex.split(subprocess.run(["just", "--justfile", str(REPO / "justfile"), "--evaluate", "CLIPPY_LINTS"],
                                          capture_output=True, text=True, check=True).stdout)
# The lint sets of the record's clippy calls: none, the justfile's, or the justfile's and a second one.
LINT_SETS = {"none": [], "one": [CLIPPY_LINTS], "second": [CLIPPY_LINTS, ["-D", "warnings"]]}
EXCEPTION_CALL = ["check", "--target", "wasm32-unknown-unknown", "-p", "c0"]
# Which recorded call uses the twin dirs: the test call that uses the first dirs too, another test call,
# the clippy calls only (check mode against the test call's build mode), an excepted call, or none.
USERS = ("same call", "other call", "check mode", "exception", "none")

# Fixed cases from the integration gate's record, run before the generated ones.
EXAMPLES = [
    # ahash <- once_cell <- portable-atomic: one `cargo test --no-run` builds both chains, which differ in the
    # features of portable-atomic.
    {"n": 3, "difference": "features", "twin_kind": "same", "user": "same call", "lints": "one", "twin_from": 0,
     "baseline": None},
    # aegis's build script run: clippy uses one dir, the test builds the other; no fingerprint field differs.
    {"n": 1, "difference": "wrapper", "twin_kind": "same", "user": "check mode", "lints": "one", "twin_from": 0,
     "baseline": None},
    # Outside both: two test builds of one crate in two feature sets.
    {"n": 2, "difference": "features", "twin_kind": "same", "user": "other call", "lints": "one", "twin_from": 1,
     "baseline": None},
    # heapless in the integration record: clippy and the test build differ in features, so not the same unit.
    {"n": 1, "difference": "features", "twin_kind": "same", "user": "check mode", "lints": "one", "twin_from": 0,
     "baseline": None},
    # Example 2 against a baseline of other pairs: a pair outside the baseline still fails.
    {"n": 2, "difference": "features", "twin_kind": "same", "user": "other call", "lints": "one", "twin_from": 1,
     "baseline": "other"},
    # Example 2 against a baseline that holds its pair and one pair the run does not see: listed, not failed.
    {"n": 2, "difference": "features", "twin_kind": "same", "user": "other call", "lints": "one", "twin_from": 1,
     "baseline": "match"},
]


def draw(rng):
    """A case: a dependency chain c0 <- c1 <- ... and maybe a second dir of its top crates, which the record
    (if any) shows used by one of USERS."""
    c = {"n": rng.randint(1, 5), "difference": rng.choice(DIFFERENCES),
         "twin_kind": rng.choice(("same", "same", "mode", "stale")),
         "user": rng.choice(USERS) if rng.random() < 0.6 else None}
    c["lints"] = rng.choice(("one", "second") if c["user"] == "check mode" else sorted(LINT_SETS))
    c["twin_from"] = rng.randrange(c["n"] + 1)
    c["baseline"] = rng.choice((None, "match", "other"))
    return c


def build(c, rng):
    target = Path(tempfile.mkdtemp(prefix="count-hash-folders-")) / "target"
    target.mkdir()
    (target / ".gc-mark").write_text("")
    os.utime(target / ".gc-mark", (BEFORE_MARK + 1, BEFORE_MARK + 1))
    first, twins = [], []
    prev_a = prev_b = None
    for i in range(c["n"]):
        crate = "c%d" % i
        va = rng.getrandbits(63)
        first.append(unit(target, crate, rng, va, [("c%d" % (i - 1), prev_a)] if i else [], AFTER_MARK))
        vb = va
        if i >= c["twin_from"]:
            vb = rng.getrandbits(63)
            kw = {}
            if i == c["twin_from"]:
                kw = {"features": {"features": ("default", "extra")}, "rustflags": {"rustflags": ("-Cx",)},
                      "env": {"env": "abc"}, "wrapper": {}}[c["difference"]]
            if c["twin_kind"] == "mode":
                kw["profile"] = BUILD
            twins.append(unit(target, crate, rng, vb, [("c%d" % (i - 1), prev_b)] if i else [],
                              BEFORE_MARK if c["twin_kind"] == "stale" else AFTER_MARK, **kw))
        prev_a, prev_b = va, vb
    env = dict(os.environ)
    if c["user"]:
        clippy = [["clippy", "--workspace", "--all-targets", "--", *lints] for lints in LINT_SETS[c["lints"]]]
        uses = [(TEST_CALL, first + (twins if c["user"] == "same call" else []))]
        uses += [(OTHER_TEST_CALL, twins)] if c["user"] == "other call" else []
        uses += [(EXCEPTION_CALL, twins)] if c["user"] == "exception" else []
        uses += [(call, twins if c["user"] == "check mode" else []) for call in clippy]
        answers = {" ".join(call): "".join(json.dumps(m) + "\n" for m in [
            {"reason": "compiler-artifact", "fresh": True, "filenames": [str(next((d / "out").iterdir()))]} for d in dirs
        ] + [{"reason": "build-finished", "success": True}]) for call, dirs in uses}
        tools = target.parent / "tools"
        tools.mkdir()
        (tools / "cargo").write_text(FAKE_CARGO)
        (tools / "cargo").chmod(0o755)
        (target.parent / "answers.json").write_text(json.dumps(answers))
        env["FAKE_CARGO_ANSWERS"] = str(target.parent / "answers.json")
        env["PATH"] = "%s:%s" % (tools, env["PATH"])
        (target.parent / "calls.jsonl").write_text("".join(json.dumps(
            {"argv": a, "env": {"HOLON_BUILD_SHA": "0123abcd", "HOLON_BUILD_TIME": None}}) + "\n" for a, _ in uses))
    return target, env


# The leaf cause of the twin crate's pair, as `pair_key` orders the two dirs.
CAUSES = {"features": "features ['extra'] vs []", "rustflags": "rustflags", "env": "build script env {'SHA': 'abc'} vs {}"}
UNSEEN = "debug/zz lib-zz: zz: rustflags"


def baseline(c):
    """The baseline lines of the case: the key of every twin pair ("match", plus one pair no run sees), or keys
    of the twin crates with another cause ("other")."""
    if c["baseline"] is None or c["difference"] == "wrapper":
        return []
    k = c["twin_from"]
    chain = lambda j: " <- ".join("c%d" % i for i in range(j, k - 1, -1))
    cause = CAUSES[c["difference"]] if c["baseline"] == "match" else "declared_features"
    return ["debug/c%d lib-c%d: %s: %s" % (j, j, chain(j), cause) for j in range(k, c["n"])] + (
        [UNSEEN] if c["baseline"] == "match" else [])


def check_committed_baseline():
    """Every line of scripts/compile-shape-baseline.txt is a pair key without a hash dir name."""
    lines = [line for line in (REPO / "scripts" / "compile-shape-baseline.txt").read_text().splitlines()
             if line.strip() and not line.startswith("#")]
    assert lines, "the committed baseline holds no pair"
    for line in lines:
        assert re.fullmatch(r"[^/ ]+/[^/ ]+ [^ ]+: .+", line), "not a pair key: %r" % line
        assert not re.search(r"\b[0-9a-f]{16}\b", line), "a hash dir name in the baseline: %r" % line


def check_take(rng, where):
    """`take` over a run of pair keys: each key is excused as often as the baseline lists it, the rest fail."""
    loader = importlib.machinery.SourceFileLoader("count_hash_folders", str(SCRIPT))
    count = importlib.util.module_from_spec(importlib.util.spec_from_loader("count_hash_folders", loader))
    loader.exec_module(count)
    names = "abc"
    baseline = collections.Counter(rng.choice(names) for _ in range(rng.randrange(5)))
    run = [rng.choice(names) for _ in range(rng.randrange(6))]
    seen = collections.Counter()
    excused = collections.Counter(k for k in run if count.take(baseline, seen, k))
    for k in names:
        want = min(baseline[k], run.count(k))
        assert excused[k] == want, "key %s: %d excused, want %d; baseline %s, run %s, %s" % (
            k, excused[k], want, dict(baseline), run, where)


def check(c, rng, where):
    target, env = build(c, rng)
    (target.parent / "baseline.txt").write_text("".join(line + "\n" for line in baseline(c)))
    argv = [sys.executable, str(SCRIPT), str(target), "--mark", str(target / ".gc-mark"),
            "--baseline", str(target.parent / "baseline.txt")] + (
        ["--record", str(target.parent / "calls.jsonl")] if c["user"] else [])
    out = subprocess.run(argv, capture_output=True, text=True, env=env)
    lines = out.stdout.splitlines()
    second = [line for line in lines if line.startswith("SECOND SHAPE")]
    unrecorded = [line for line in lines if line.startswith("UNRECORDED")]
    lint = [line for line in lines if line.startswith("LINT ARGS")]
    where = "%s, case %s:\n%s%s" % (where, c, out.stdout, out.stderr)
    twins = ["c%d" % i for i in range(c["twin_from"], c["n"])]
    built_after = c["twin_kind"] != "stale"
    used = c["user"] not in (None, "none") or built_after
    want_unrecorded = twins if c["user"] == "none" and built_after else []
    same_unit = c["difference"] == "wrapper"
    apart = c["user"] in (None, "none", "other call") or (c["user"] == "check mode" and not same_unit)
    pairs = twins if used and c["twin_kind"] != "mode" and apart else []
    excused = pairs if c["baseline"] == "match" and c["difference"] != "wrapper" else []
    want_second = [t for t in pairs if t not in excused]
    assert len(unrecorded) == len(want_unrecorded), "want UNRECORDED for %s, " % want_unrecorded + where
    assert len(second) == len(want_second), "want SECOND SHAPE for %s, " % want_second + where
    if (c["user"] == "same call" or c["user"] == "check mode" and same_unit) and c["twin_kind"] != "mode":
        listed = [line for line in lines if line.endswith(": " + {"same call": "one call"}.get(c["user"], "check vs build"))]
        assert len(listed) == len(twins), "want the %s pair of %s listed, " % (c["user"], twins) + where
    listed = [line for line in lines if line.endswith(": baseline")]
    assert len(listed) == len(excused), "want the pairs of %s listed as baseline, " % excused + where
    lines_in = len(baseline(c))
    assert "%d pair(s) seen, %d not seen" % (len(excused), lines_in - len(excused)) in out.stdout, "baseline tally, " + where
    want_lint = 1 if c["user"] and c["lints"] == "second" else 0
    assert len(lint) == want_lint, "want %d LINT ARGS, " % want_lint + where
    assert out.returncode == (1 if second or unrecorded or lint else 0), "exit %d, " % out.returncode + where
    want = {"features": ("features [] vs ['extra']", "features ['extra'] vs []"), "rustflags": ("rustflags",),
            "env": ("build script env {} vs {'SHA': 'abc'}", "build script env {'SHA': 'abc'} vs {}"),
            "wrapper": ("no fingerprint field differs",)}[c["difference"]]
    for line in second:
        assert any("%s: %s" % (twins[0], w) in line for w in want), "cause %r missing, " % (want,) + where
    shutil.rmtree(target.parent)


def check_two_differing_deps():
    """c3's clippy dir and test dir differ in two deps: c1 only by mode, c2 in features. Not the same unit."""
    rng = random.Random(7)
    target = Path(tempfile.mkdtemp(prefix="count-hash-folders-")) / "target"
    target.mkdir()
    (target / ".gc-mark").write_text("")
    os.utime(target / ".gc-mark", (BEFORE_MARK + 1, BEFORE_MARK + 1))
    v = [rng.getrandbits(63) for _ in range(6)]
    c1a, c1b = (unit(target, "c1", rng, x, [], AFTER_MARK) for x in v[0:2])
    c2a = unit(target, "c2", rng, v[2], [], AFTER_MARK)
    c2b = unit(target, "c2", rng, v[3], [], AFTER_MARK, features=("default", "extra"))
    c3a = unit(target, "c3", rng, v[4], [("c1", v[0]), ("c2", v[2])], AFTER_MARK)
    c3b = unit(target, "c3", rng, v[5], [("c1", v[1]), ("c2", v[3])], AFTER_MARK)
    uses = [(TEST_CALL, [c1a, c2a, c3a]), (["clippy", "--workspace", "--all-targets", "--", *CLIPPY_LINTS], [c1b, c2b, c3b])]
    answers = {" ".join(call): "".join(json.dumps(m) + "\n" for m in [
        {"reason": "compiler-artifact", "fresh": True, "filenames": [str(next((d / "out").iterdir()))]} for d in dirs
    ] + [{"reason": "build-finished", "success": True}]) for call, dirs in uses}
    tools = target.parent / "tools"
    tools.mkdir()
    (tools / "cargo").write_text(FAKE_CARGO)
    (tools / "cargo").chmod(0o755)
    (target.parent / "answers.json").write_text(json.dumps(answers))
    (target.parent / "calls.jsonl").write_text("".join(json.dumps(
        {"argv": a, "env": {"HOLON_BUILD_SHA": "0123abcd", "HOLON_BUILD_TIME": None}}) + "\n" for a, _ in uses))
    (target.parent / "baseline.txt").write_text("")
    env = dict(os.environ, FAKE_CARGO_ANSWERS=str(target.parent / "answers.json"), PATH="%s:%s" % (tools, os.environ["PATH"]))
    out = subprocess.run([sys.executable, str(SCRIPT), str(target), "--mark", str(target / ".gc-mark"), "--record",
                          str(target.parent / "calls.jsonl"), "--baseline", str(target.parent / "baseline.txt")],
                         capture_output=True, text=True, env=env)
    second = sorted(line.split(",")[0] for line in out.stdout.splitlines() if line.startswith("SECOND SHAPE"))
    assert second == ["SECOND SHAPE: debug/c2 lib-c2", "SECOND SHAPE: debug/c3 lib-c3"], "two differing deps: %s\n%s" % (
        second, out.stdout)
    shutil.rmtree(target.parent)


KWARGS = {"features": {"features": ("default", "extra")}, "rustflags": {"rustflags": ("-Cx",)}, "env": {"env": "abc"},
          "wrapper": {}}
WANT = {"features": ("features [] vs ['extra']", "features ['extra'] vs []"), "rustflags": ("rustflags",),
        "env": ("build script env {} vs {'SHA': 'abc'}", "build script env {'SHA': 'abc'} vs {}"),
        "wrapper": ("no fingerprint field differs",)}
SAME_UNIT = "no fingerprint field differs (a workspace member under cargo clippy vs cargo check)"


def check_diamond(rng, where):
    """c3 depends on c1 and c2, whose clippy and test dirs each differ in a drawn field. c3's pair is the same unit
    only when both deps are; its key names both dep paths, so a baseline line naming one path does not excuse it."""
    d = {"c1": rng.choice(DIFFERENCES), "c2": rng.choice(DIFFERENCES)}
    listed_as = rng.choice((None, "first path", "every path"))
    target = Path(tempfile.mkdtemp(prefix="count-hash-folders-")) / "target"
    target.mkdir()
    (target / ".gc-mark").write_text("")
    os.utime(target / ".gc-mark", (BEFORE_MARK + 1, BEFORE_MARK + 1))
    v = [rng.getrandbits(63) for _ in range(6)]
    dirs = {"c1": (unit(target, "c1", rng, v[0], [], AFTER_MARK), unit(target, "c1", rng, v[1], [], AFTER_MARK, **KWARGS[d["c1"]])),
            "c2": (unit(target, "c2", rng, v[2], [], AFTER_MARK), unit(target, "c2", rng, v[3], [], AFTER_MARK, **KWARGS[d["c2"]]))}
    dirs["c3"] = (unit(target, "c3", rng, v[4], [("c1", v[0]), ("c2", v[2])], AFTER_MARK),
                  unit(target, "c3", rng, v[5], [("c1", v[1]), ("c2", v[3])], AFTER_MARK))
    uses = [(TEST_CALL, [a for a, _ in dirs.values()]),
            (["clippy", "--workspace", "--all-targets", "--", *CLIPPY_LINTS], [b for _, b in dirs.values()])]
    answers = {" ".join(call): "".join(json.dumps(m) + "\n" for m in [
        {"reason": "compiler-artifact", "fresh": True, "filenames": [str(next((x / "out").iterdir()))]} for x in ds
    ] + [{"reason": "build-finished", "success": True}]) for call, ds in uses}
    tools = target.parent / "tools"
    tools.mkdir()
    (tools / "cargo").write_text(FAKE_CARGO)
    (tools / "cargo").chmod(0o755)
    (target.parent / "answers.json").write_text(json.dumps(answers))
    (target.parent / "calls.jsonl").write_text("".join(json.dumps(
        {"argv": a, "env": {"HOLON_BUILD_SHA": "0123abcd", "HOLON_BUILD_TIME": None}}) + "\n" for a, _ in uses))
    leaf = lambda k: CAUSES.get(d[k], SAME_UNIT)
    lines = {None: [], "first path": ["debug/c3 lib-c3: c3 <- c1: %s" % leaf("c1")],
             "every path": ["debug/c3 lib-c3: c3 <- c1: %s; c3 <- c2: %s" % (leaf("c1"), leaf("c2"))]}[listed_as]
    (target.parent / "baseline.txt").write_text("".join(line + "\n" for line in lines))
    env = dict(os.environ, FAKE_CARGO_ANSWERS=str(target.parent / "answers.json"), PATH="%s:%s" % (tools, os.environ["PATH"]))
    out = subprocess.run([sys.executable, str(SCRIPT), str(target), "--mark", str(target / ".gc-mark"), "--record",
                          str(target.parent / "calls.jsonl"), "--baseline", str(target.parent / "baseline.txt")],
                         capture_output=True, text=True, env=env)
    where = "%s, deps %s, baseline %s:\n%s" % (where, d, listed_as, out.stdout)
    drifted = [k for k in ("c1", "c2") if d[k] != "wrapper"]
    c3_excused = bool(drifted) and listed_as == "every path"
    want = sorted(drifted + (["c3"] if drifted and not c3_excused else []))
    second = [line for line in out.stdout.splitlines() if line.startswith("SECOND SHAPE")]
    assert sorted(line.split(" ")[2].split("/")[1] for line in second) == want, "want SECOND SHAPE for %s, " % want + where
    for line in second:
        if line.startswith("SECOND SHAPE: debug/c3 "):
            for k in ("c1", "c2"):
                assert any("c3 <- %s: %s" % (k, w) in line for w in WANT[d[k]]), "dep path %s missing, " % k + where
    assert "%d pair(s) seen" % c3_excused in out.stdout, "baseline tally, " + where
    shutil.rmtree(target.parent)


def main():
    check_committed_baseline()
    check_two_differing_deps()
    examples = [] if "SEED" in os.environ else [int(os.environ["EXAMPLE"])] if "EXAMPLE" in os.environ else range(len(EXAMPLES))
    seeds = [] if "EXAMPLE" in os.environ else [int(os.environ["SEED"])] if "SEED" in os.environ else range(200)
    for i in examples:
        check(EXAMPLES[i], random.Random(i), "example %d" % i)
    for seed in seeds:
        rng = random.Random(seed)
        check(draw(rng), rng, "seed %d" % seed)
        check_take(rng, "seed %d" % seed)
        check_diamond(rng, "seed %d" % seed)
    print("test-count-hash-folders: %d example(s) and %d generated case(s) ok" % (len(examples), len(seeds)))


if __name__ == "__main__":
    main()
