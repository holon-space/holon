#!/usr/bin/env python3
"""Properties of scripts/python311, the one Python >= 3.11 picker, over drawn sets of interpreters on PATH.

Run: `scripts/python311 scripts/test-python311.py` (CASES=<n>, SEED=<s>). Needs Python >= 3.11: the stubs that report
3.11 or newer run scripts under this interpreter, and archlint imports tomllib.
PATH holds only stub interpreters (scripts/python_stubs.py) and the shell tools, never a host Python. For each draw:
- the interpreter that ran the script is the first of python3.14 .. python3.11, python3 present with version >= 3.11;
- with none such, the exit is 127 and the message names 3.11;
- the picker's exit is the script's own;
- `archlint/archlint --help` runs under the same interpreter, so the wrapper uses the shared picker.
Generators: each candidate is absent or present with a version from 3.9 to 3.14. The fixed examples (an old first
python3 beside a python3.12, only an old python3, only a new python3, all five present) run first.
"""
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

from python_stubs import CANDIDATES, picked, tool_dir, write_stubs

assert sys.version_info >= (3, 11), f"needs Python >= 3.11, running {sys.version}"

REPO = Path(__file__).resolve().parent.parent
PICKER = REPO / "scripts/python311"
ARCHLINT = REPO / "archlint/archlint"
VERSIONS = [(3, 9), (3, 10), (3, 11), (3, 12), (3, 13), (3, 14)]
FIXED = {
    "an old first python3 beside a python3.12": ({"python3": (3, 9), "python3.12": (3, 12)}, 0),
    "only an old python3": ({"python3": (3, 9)}, 0),
    "only a new python3": ({"python3": (3, 13)}, 3),
    "all five present": ({c: (3, 12) for c in CANDIDATES}, 1),
}


def run(argv, versions, scratch):
    """(exit, stderr, names of the stubs that ran a script) of `argv` under a PATH of `versions` stubs only."""
    with tempfile.TemporaryDirectory(dir=scratch) as d:
        d = Path(d)
        record = d / "record"
        record.write_text("")
        write_stubs(d / "bin", versions, record)
        env = {**os.environ, "PATH": f"{d / 'bin'}:{scratch / 'tools'}"}
        r = subprocess.run(argv, env=env, capture_output=True, text=True)
        return r.returncode, r.stderr, record.read_text().split()


def check(name, versions, code, scratch, script):
    want = picked(versions)
    failures = []
    rc, err, ran = run([str(PICKER), str(script), str(code)], versions, scratch)
    if want is None:
        if rc != 127 or "3.11" not in err or ran:
            failures.append(f"no interpreter >= 3.11: exit {rc}, ran {ran}, stderr {err!r}")
    elif ran != [want] or rc != code:
        failures.append(f"ran {ran} with exit {rc}, want {[want]} with the script's exit {code}; stderr {err!r}")
    arc, aerr, aran = run([str(ARCHLINT), "--help"], versions, scratch)
    if aran != ran or arc != (127 if want is None else 0):
        failures.append(f"archlint --help ran {aran} with exit {arc}, the picker ran {ran}; stderr {aerr!r}")
    return [f"{name} {versions}: {f}" for f in failures]


def main():
    cases = int(os.environ.get("CASES", "40"))
    seed = int(os.environ.get("SEED", random.randrange(2**32)))
    rng = random.Random(seed)
    print(f"seed {seed}, {cases} drawn cases")
    failures = []
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        tool_dir(scratch / "tools")
        script = scratch / "exit_with.py"
        script.write_text("import sys\nsys.exit(int(sys.argv[1]))\n")
        for name, (versions, code) in FIXED.items():
            failures += check(name, versions, code, scratch, script)
        for i in range(cases):
            versions = {c: rng.choice(VERSIONS) for c in CANDIDATES if rng.random() < 0.5}
            failures += check(f"draw {i}", versions, rng.randrange(4), scratch, script)
    for failure in failures:
        print(f"FAIL: {failure}")
    print(f"test-python311: {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
