#!/usr/bin/python3
"""Property: a script that a gate step's recipe runs adds no second call of a recipe to the gate's just record.

    scripts/test-just-record-nested.py [cases]

The integrate gate records the step recipe R itself; R's body then runs its scripts with the record and the
shim inherited. The record of R plus those scripts must pass scripts/check-just-calls-once.py.
"""
import json
import os
import random
import re
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
SHIMS = os.path.join(REPO, "scripts", "shims")
STEPS = ["gate-arch", "arch-validate", "cargo-free-checks"]


def recipe_scripts(recipe):
    body = subprocess.run(["just", "--show", recipe], cwd=REPO, check=True, capture_output=True, text=True).stdout
    return sorted(set(re.findall(r"\./scripts/(test-[\w-]+\.py)", body)))


def draw_gate_step_script(rng):
    pairs = [(r, s) for r in STEPS for s in recipe_scripts(r)]
    return rng.choice(pairs)


def run(recipe, script):
    with tempfile.TemporaryDirectory() as d:
        rec = os.path.join(d, "rec.jsonl")
        with open(rec, "w") as f:
            f.write(json.dumps({"argv": [recipe]}) + "\n")
        env = {**os.environ, "HOLON_JUST_RECORD": rec, "PATH": SHIMS + ":" + os.environ["PATH"]}
        subprocess.run([os.path.join(REPO, "scripts", script)], cwd=REPO, env=env, capture_output=True)
        r = subprocess.run([sys.executable, os.path.join(REPO, "scripts", "check-just-calls-once.py"), rec],
                           capture_output=True, text=True)
        assert r.returncode == 0, f"{recipe} -> {script}: {r.stdout.strip()}"


def main():
    cases = int(sys.argv[1]) if len(sys.argv) > 1 else 5
    run("gate-arch", "test-gate-arch-verdicts.py")
    rng = random.Random(int(os.environ.get("SEED", "1")))
    for _ in range(cases):
        run(*draw_gate_step_script(rng))
    print("nested just record: ok")


main()
