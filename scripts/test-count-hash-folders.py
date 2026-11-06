#!/usr/bin/env python3
"""Self-check for scripts/count-hash-folders.py on generated fake target dirs.

Run: `python3 scripts/test-count-hash-folders.py`. Exits non-zero on the first violation.
COUNT_HASH_FOLDERS=<path> runs the checks against another copy of the script. SEED=<n> replays the generated cases.
"""
import json
import os
import random
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


FAKE_CARGO = """#!/usr/bin/python3
import os, sys
sys.stdout.write(open(os.environ["FAKE_CARGO_" + ("EXCEPTION" if "--target" in sys.argv else "CANON")]).read())
"""
CANON_CALL = ["test", "--workspace"]
CLIPPY_LINTS = shlex.split(subprocess.run(["just", "--justfile", str(REPO / "justfile"), "--evaluate", "CLIPPY_LINTS"],
                                          capture_output=True, text=True, check=True).stdout)
# The lint sets of the record's clippy calls: none, the justfile's, or the justfile's and a second one.
LINT_SETS = {"none": [], "one": [CLIPPY_LINTS], "second": [CLIPPY_LINTS, ["-D", "warnings"]]}
EXCEPTION_CALL = ["check", "--target", "wasm32-unknown-unknown", "-p", "c0"]


def case(rng):
    """A target with a dependency chain c0 <- c1 <- ... and maybe a second dir of its top crates, which the
    record (if any) shows used by a CANON call, by an excepted call, or by none."""
    target = Path(tempfile.mkdtemp(prefix="count-hash-folders-")) / "target"
    target.mkdir()
    (target / ".gc-mark").write_text("")
    os.utime(target / ".gc-mark", (BEFORE_MARK + 1, BEFORE_MARK + 1))
    c = {"n": rng.randint(1, 5), "difference": rng.choice(DIFFERENCES),
         "twin_kind": rng.choice(("same", "same", "mode", "stale")),
         "user": rng.choice(("canon", "exception", "none")) if rng.random() < 0.5 else None,
         "lints": rng.choice(sorted(LINT_SETS))}
    c["crates"] = ["c%d" % i for i in range(c["n"])]
    c["twin_from"] = rng.randrange(c["n"] + 1)
    used = {"canon": [], "exception": [], "none": []}
    prev_a = prev_b = None
    for i, crate in enumerate(c["crates"]):
        va = rng.getrandbits(63)
        used["canon"].append(unit(target, crate, rng, va, [("c%d" % (i - 1), prev_a)] if i else [], AFTER_MARK))
        vb = va
        if i >= c["twin_from"]:
            vb = rng.getrandbits(63)
            kw = {}
            if i == c["twin_from"]:
                kw = {"features": {"features": ("default", "extra")}, "rustflags": {"rustflags": ("-Cx",)},
                      "env": {"env": "abc"}, "wrapper": {}}[c["difference"]]
            if c["twin_kind"] == "mode":
                kw["profile"] = BUILD
            used[c["user"] or "none"].append(unit(target, crate, rng, vb, [("c%d" % (i - 1), prev_b)] if i else [],
                                                  BEFORE_MARK if c["twin_kind"] == "stale" else AFTER_MARK, **kw))
        prev_a, prev_b = va, vb
    env = dict(os.environ)
    if c["user"]:
        tools = target.parent / "tools"
        tools.mkdir()
        (tools / "cargo").write_text(FAKE_CARGO)
        (tools / "cargo").chmod(0o755)
        for name, dirs in (("CANON", used["canon"]), ("EXCEPTION", used["exception"])):
            msgs = [{"reason": "compiler-artifact", "fresh": True, "filenames": [str(next((d / "out").iterdir()))]}
                    for d in dirs] + [{"reason": "build-finished", "success": True}]
            (target.parent / name).write_text("".join(json.dumps(m) + "\n" for m in msgs))
            env["FAKE_CARGO_" + name] = str(target.parent / name)
        env["PATH"] = "%s:%s" % (tools, env["PATH"])
        calls = [CANON_CALL] + ([EXCEPTION_CALL] if used["exception"] else []) + [
            ["clippy", "--workspace", "--all-targets", "--", *lints] for lints in LINT_SETS[c["lints"]]]
        (target.parent / "calls.jsonl").write_text("".join(json.dumps(
            {"argv": a, "env": {"HOLON_BUILD_SHA": "0123abcd", "HOLON_BUILD_TIME": None}}) + "\n" for a in calls))
    return target, c, env


def check(seed):
    rng = random.Random(seed)
    target, c, env = case(rng)
    argv = [sys.executable, str(SCRIPT), str(target), "--mark", str(target / ".gc-mark")] + (
        ["--record", str(target.parent / "calls.jsonl")] if c["user"] else [])
    out = subprocess.run(argv, capture_output=True, text=True, env=env)
    lines = out.stdout.splitlines()
    second = [line for line in lines if line.startswith("SECOND SHAPE")]
    unrecorded = [line for line in lines if line.startswith("UNRECORDED")]
    lint = [line for line in lines if line.startswith("LINT ARGS")]
    where = "seed %d, case %s:\n%s%s" % (seed, c, out.stdout, out.stderr)
    twins = c["crates"][c["twin_from"]:]
    built_after = c["twin_kind"] != "stale"
    used = c["user"] in ("canon", "exception") or built_after
    want_unrecorded = twins if c["user"] == "none" and built_after else []
    want_second = twins if used and c["twin_kind"] != "mode" and c["user"] != "exception" else []
    assert len(unrecorded) == len(want_unrecorded), "want UNRECORDED for %s, " % want_unrecorded + where
    assert len(second) == len(want_second), "want SECOND SHAPE for %s, " % want_second + where
    want_lint = 1 if c["user"] and c["lints"] == "second" else 0
    assert len(lint) == want_lint, "want %d LINT ARGS, " % want_lint + where
    assert out.returncode == (1 if second or unrecorded or lint else 0), "exit %d, " % out.returncode + where
    want = {"features": ("features [] vs ['extra']", "features ['extra'] vs []"), "rustflags": ("rustflags",),
            "env": ("build script env {} vs {'SHA': 'abc'}", "build script env {'SHA': 'abc'} vs {}"),
            "wrapper": ("no fingerprint field differs",)}[c["difference"]]
    for line in second:
        assert any("%s: %s" % (twins[0], w) in line for w in want), "cause %r missing, " % (want,) + where
    shutil.rmtree(target.parent)


def main():
    seeds = [int(os.environ["SEED"])] if "SEED" in os.environ else range(200)
    for seed in seeds:
        check(seed)
    print("test-count-hash-folders: %d case(s) ok" % len(seeds))


if __name__ == "__main__":
    main()
