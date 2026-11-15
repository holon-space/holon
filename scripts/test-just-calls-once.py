#!/usr/bin/python3
"""Property test for scripts/check-just-calls-once.py and the live test of scripts/shims/just.

    scripts/test-just-calls-once.py [cases]

SEED=<n> replays a run. Properties:
- the check exits 1 iff some recipe call (same recipe and args, whatever justfile/directory options or env
  prefix it carries) occurs twice, and its FAIL lines carry exactly the drawn counts;
- calls that run no recipe never count;
- the shim records the argv it got and runs the next `just` with that argv and its exit code; with
  HOLON_JUST_RECORD unset or HOLON_JUST_RECORD_ARMED unset it records nothing, and the just it starts is
  not armed, so a `just` call below it records nothing.
"""
import collections
import json
import os
import random
import stat
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
CHECK = os.path.join(REPO, "scripts", "check-just-calls-once.py")
SHIM_DIR = os.path.join(REPO, "scripts", "shims")
NO_RECIPE = ["--evaluate", "--dump", "--show", "--list", "--summary", "--variables"]
RECIPES = ["cargo-free-checks", "gate-arch", "deny", "machete", "gate-compile", "windowed-gate"]
ARGS = ["", "a", "--flag", "x y"]

# (calls, expected exit, expected FAIL lines); each call is (argv, key-or-None).
STATIC = [
    ([["cargo-free-checks"], ["gate-arch"], ["cargo-free-checks"]], 1, {"cargo-free-checks": 2}),
    ([["--evaluate", "A"], ["--evaluate", "A"]], 0, {}),
    ([["--justfile", "j", "r"], ["r"]], 1, {"r": 2}),
]


def run_check(argvs):
    with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as f:
        for a in argvs:
            f.write(json.dumps({"argv": a}) + "\n")
    try:
        p = subprocess.run([sys.executable, CHECK, f.name], capture_output=True, text=True)
    finally:
        os.unlink(f.name)
    return p.returncode, p.stdout


def draw_call(rng):
    recipe = rng.choice(RECIPES)
    args = [a for a in ARGS if a and rng.random() < 0.3]
    argv = [recipe, *args]
    if rng.random() < 0.3:
        argv = [rng.choice(["--justfile", "-f", "--working-directory", "-d"]), rng.choice(["j1", "j2"]), *argv]
    if rng.random() < 0.15:
        argv = [rng.choice(NO_RECIPE), *argv[:1]]
        return argv, None
    key = " ".join([recipe, *args])
    return argv, key


def check_case(calls):
    expected = collections.Counter(k for _, k in calls if k is not None)
    rc, out = run_check([a for a, _ in calls])
    dup = {k: n for k, n in expected.items() if n > 1}
    got = {}
    for line in out.splitlines():
        if line.startswith("FAIL: just "):
            body, _, n = line[len("FAIL: just "):].rpartition(" ran ")
            got[body] = int(n.split()[0])
    assert rc == (1 if dup else 0), (rc, out, calls)
    assert got == dup, (got, dup, calls)
    if not dup:
        assert out.strip() == "just-calls-once: %d calls, 0 repeated" % sum(expected.values()), out


def shim_test():
    with tempfile.TemporaryDirectory() as d:
        stub = os.path.join(d, "bin", "just")
        os.makedirs(os.path.dirname(stub))
        with open(stub, "w") as f:
            f.write('#!/bin/sh\necho "stub $*"\nexit 7\n')
        os.chmod(stub, os.stat(stub).st_mode | stat.S_IXUSR)
        rec = os.path.join(d, "rec", "just.jsonl")
        env = {**os.environ, "PATH": os.pathsep.join([SHIM_DIR, os.path.dirname(stub), os.environ["PATH"]])}
        env.pop("HOLON_JUST_RECORD", None)
        env.pop("HOLON_JUST_RECORD_ARMED", None)
        p = subprocess.run(["just", "r", "a b"], env=env, capture_output=True, text=True)
        assert (p.returncode, p.stdout) == (7, "stub r a b\n"), p
        assert not os.path.exists(rec)
        p = subprocess.run(["just", "r", "a b"], env={**env, "HOLON_JUST_RECORD": rec}, capture_output=True, text=True)
        assert (p.returncode, p.stdout) == (7, "stub r a b\n"), p
        assert not os.path.exists(rec)
        p = subprocess.run(["just", "r", "a b"], env={**env, "HOLON_JUST_RECORD": rec, "HOLON_JUST_RECORD_ARMED": "1"},
                           capture_output=True, text=True)
        assert (p.returncode, p.stdout) == (7, "stub r a b\n"), p
        assert [json.loads(line) for line in open(rec)] == [{"argv": ["r", "a b"]}]


def main():
    cases = int(sys.argv[1]) if len(sys.argv) > 1 else 200
    seed = int(os.environ.get("SEED", random.randrange(10**9)))
    print("SEED=%d" % seed)
    for calls, rc, dup in STATIC:
        keyed = [(a, None if a[0] in NO_RECIPE else " ".join(a[2:] if a[0] == "--justfile" else a)) for a in calls]
        check_case(keyed)
    rng = random.Random(seed)
    for _ in range(cases):
        base = [draw_call(rng) for _ in range(rng.randrange(0, 8))]
        calls = base + [rng.choice(base) for _ in range(rng.randrange(0, 3))] if base else base
        rng.shuffle(calls)
        check_case(calls)
    shim_test()
    print("ok: %d cases + %d static + shim" % (cases, len(STATIC)))


main()
