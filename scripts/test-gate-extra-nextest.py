#!/usr/bin/env python3
"""Property check for the extra-nextest step of scripts/gate-integrate-steps.sh: a real nextest run over the
fixture crate of scripts/nextest_fixture.py.

Run: `BASE=<commit> python3 scripts/test-gate-extra-nextest.py [cases]`. Exits non-zero on the first violation.
Generators: GATE_STEP_ARGS, a filterset that nextest parses or one it cannot, each class known by construction; and
the plan (each test P, K or U).
Properties, per case:
- an unparsable filter ends the step with exit 2 and one line naming the args, before any test starts and before
  the known-reds classifier;
- a parsable filter, and empty or unset args, give the exit and the ran tests of the step at BASE, with the same
  plan.
Fixed filters run first through the same properties.
"""
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

from nextest_fixture import REPO, TESTS, fixture, known_red_wording, ran_tests

SUT = REPO / "scripts" / "gate-integrate-steps.sh"
GATE = "g"
WORDS = ["one", "two", "three", "flaky", "known", "red", "org", "suite", "timing", "rerun"]
OPTIONS = ["-x", "--help", "--workspace", "-E", "--no-tests=fail"]
# (args, parsable); None = GATE_STEP_ARGS unset.
EXAMPLES = [("two three", False), ("package(nope)", False), ("package(fixture) |", False), ("--help", False),
            ("-x", False), ("  ", False), ("test(=t00) | test(=t01)", True), ("", True), (None, True)]


def draw_valid(rng, depth=0):
    if depth == 3 or rng.random() < 0.4:
        return rng.choice([f"test(={rng.choice(TESTS)})", "test(/^t0/)", "package(fixture)", "all()", "none()"])
    kind = rng.choice(["|", "&", "not", "()"])
    if kind == "not":
        return f"not {draw_valid(rng, depth + 1)}"
    if kind == "()":
        return f"({draw_valid(rng, depth + 1)})"
    return f"{draw_valid(rng, depth + 1)} {kind} {draw_valid(rng, depth + 1)}"


def draw_invalid(rng):
    valid = draw_valid(rng)
    kind = rng.randrange(6)
    if kind == 0:
        return " ".join(rng.choices(WORDS, k=rng.randint(1, 4)))
    if kind == 1:
        return f"{valid} {rng.choice(WORDS)}"
    if kind == 2:
        i = valid.rindex(")")
        return valid[:i] + valid[i + 1:]
    if kind == 3:
        return f"{valid} {rng.choice('|&')}"
    if kind == 4:
        return f"package({rng.choice(['nope', 'fixtures', 'holon'])})"
    return " ".join([rng.choice(OPTIONS), *rng.sample([valid, *WORDS], rng.randint(0, 2))])


def run_step(root, script, args, plan):
    logs = root / "target/gate-logs"
    for f in logs.iterdir():
        f.unlink()
    env = {**os.environ, "GATE": GATE, "PBT_FEATURES": "pbt", "PLAN": plan, "CARGO_TARGET_DIR": str(root / "target")}
    env.pop("GATE_STEP_ARGS", None)
    if args is not None:
        env["GATE_STEP_ARGS"] = args
    r = subprocess.run(["bash", str(script), "extra-nextest"], cwd=root, env=env, capture_output=True, text=True)
    return r, sorted(ran_tests((logs / f"{GATE}-extra.log").read_text()))


def check(root, base, args, parsable, plan):
    r, ran = run_step(root, SUT, args, plan)
    log = f"args={args!r} plan={plan}\nexit {r.returncode}\n{r.stdout}\n{r.stderr}"
    if parsable:
        b, base_ran = run_step(root, base, args, plan)
        assert (r.returncode, ran) == (b.returncode, base_ran), \
            f"{log}\nran {ran}, at BASE: exit {b.returncode}, ran {base_ran}\n{b.stdout}\n{b.stderr}"
        return
    lines = r.stdout.splitlines()
    named = [l for l in lines if l.startswith("extra-nextest: ")]
    assert r.returncode == 2, f"{log}\nwant exit 2"
    assert named == [f"extra-nextest: GATE_STEP_ARGS is no valid filterset: {args}"], f"{log}\nnamed {named}"
    assert not [l for l in lines if l.startswith("[known-reds]")], f"{log}\nthe classifier ran"
    assert not (root / f"target/gate-logs/{GATE}-extra-classify.txt").exists(), f"{log}\na classify file exists"
    assert ran == [], f"{log}\ntests started: {ran}"


def main():
    cases = int(sys.argv[1]) if len(sys.argv) > 1 else 24
    rng = random.Random(int(os.environ.get("SEED", random.randrange(1 << 30))))
    generated = [(draw_valid(rng), True) if rng.random() < 0.5 else (draw_invalid(rng), False) for _ in range(cases)]
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "fixture"
        root.mkdir()
        fixture(root, known_red_wording())
        with (root / "Cargo.toml").open("a") as f:
            f.write('\n[features]\npbt = []\n')
        (root / ".config").mkdir()
        (root / ".config/nextest.toml").write_text("[profile.load-tolerant]\n")
        # The step calls the classifier and the leftovers check by paths relative to the checkout root.
        (root / "scripts").symlink_to(REPO / "scripts")
        (root / "docs").symlink_to(REPO / "docs")
        (root / "target/gate-logs").mkdir(parents=True)
        base = Path(tmp) / "base-steps.sh"
        base.write_bytes(subprocess.run(["jj", "file", "show", "-r", os.environ["BASE"], "scripts/gate-integrate-steps.sh"],
                                        cwd=REPO, capture_output=True, check=True).stdout)
        subprocess.run(["cargo", "build", "--tests", "--features", "pbt", "--quiet"], cwd=root, check=True,
                       env={**os.environ, "CARGO_TARGET_DIR": str(root / "target")})
        for args, parsable in EXAMPLES + generated:
            plan = "".join(rng.choice("PPKU") for _ in TESTS)
            check(root, base, args, parsable, plan)
            print(f"ok {'parsable' if parsable else 'unparsable'} args={args!r} plan={plan}", flush=True)
    print("ok")


main()
