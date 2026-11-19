#!/usr/bin/env python3
"""Properties of the twin-name check of scripts/twin-own-red.sh, over drawn twin tables.

Run: `python3 scripts/test-twin-own-red.py` (CASES=<n>, SEED=<s>).
Each case builds a temp tree with a copy of the script whose TABLE holds drawn names, and a drawn
crates/holon-net/tests/exec_support/twin_names.rs, then runs `scripts/twin-own-red.sh none`: the filter `none`
matches no row, so only the name check runs (a stub `cargo` on PATH records any call; there must be none).
Properties, per case, with the expected verdict computed from the drawn sets:
- (a) the lowering names TABLE does not hold are exactly the names of the "does not name" FAIL line;
- (b) the TABLE names in neither the executor nor the lowering table are exactly the names of the "does not hold" FAIL line;
- (c) the exit is 1 iff (a) or (b) names a name, else 0, and no other FAIL line is printed.
Generators: names over [a-z0-9_-] (never `none`), 1-6 per Rust table, each row written on one line or wrapped as
rustfmt wraps it; TABLE is a drawn subset of both tables' names plus 0-2 names neither holds.
The fixed examples run first through the same properties.
"""
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCRIPT = Path("scripts/twin-own-red.sh")
NAMES = Path("crates/holon-net/tests/exec_support/twin_names.rs")
ALPHABET = "abcdefghijklmnopqrstuvwxyz0123456789_-"
MISSING = "names lowering twins the table does not name: "
UNKNOWN = "does not hold: "

# (name, wrapped) rows of TWINS, of LOWER_TWINS, and the TABLE names.
EXAMPLES = {
    "a lowering name with a digit, not in TABLE": ([("no-mark", False)], [("double-answer2", False)], []),
    "a lowering name with an underscore, not in TABLE": ([("no-mark", False)], [("ask_while_x", False)], []),
    "a wrapped lowering row, not in TABLE": ([("no-mark", False)], [("ask-while-outcome-unknown", True)], []),
    "a wrapped lowering row in TABLE": ([("no-mark", False)], [("ask-while-outcome-unknown", True)],
                                        ["ask-while-outcome-unknown"]),
    "an executor twin in TABLE": ([("no-quiescence", False), ("keep-work-failed", True)], [("double-answer", False)],
                                  ["double-answer", "no-quiescence", "keep-work-failed"]),
    "a TABLE row naming no twin": ([("no-mark", False)], [("double-answer", False)], ["double-answer", "gone-twin"]),
    "a TABLE row with a digit naming no twin": ([("no-mark", False)], [("double-answer", False)],
                                                ["double-answer", "double-answer2"]),
}


def row(name, variant, kind, wrapped):
    if wrapped:
        return f'    (\n        "{name}",\n        {kind}::{variant},\n    ),\n'
    return f'    ("{name}", {kind}::{variant}),\n'


def names_file(twins, lowers):
    ex = "".join(row(n, f"E{i}", "Twin", w) for i, (n, w) in enumerate(twins))
    lo = "".join(row(n, f"L{i}", "LowerTwin", w) for i, (n, w) in enumerate(lowers))
    return ("use holon_net::exec::Twin;\nuse holon_net::uft::LowerTwin;\n\n"
            f"const TWINS: &[(&str, Twin)] = &[\n{ex}];\n\n"
            f"const LOWER_TWINS: &[(&str, LowerTwin)] = &[\n{lo}];\n\n"
            "fn listed(t: Twin) -> Twin {\n    match t {\n        Twin::E0 => t,\n    }\n}\n")


def script_with_table(table):
    text = (REPO / SCRIPT).read_text()
    rows = "".join(f"{n}|marker of {n}\n" for n in table)
    new, count = re.subn(r"^TABLE='\n.*?^'\n", lambda _: f"TABLE='\n{rows}'\n", text, flags=re.S | re.M)
    assert count == 1, f"{SCRIPT} holds no single TABLE='...' block"
    return new


def named(line, marker):
    return set(line.split(marker, 1)[1].split())


def check(label, twins, lowers, table):
    lower_names = {n for n, _ in lowers}
    known = lower_names | {n for n, _ in twins}
    want_missing = lower_names - set(table)
    want_unknown = set(table) - known
    with tempfile.TemporaryDirectory() as d:
        root = Path(d)
        (root / SCRIPT).parent.mkdir(parents=True)
        (root / SCRIPT).write_text(script_with_table(table))
        (root / NAMES).parent.mkdir(parents=True)
        (root / NAMES).write_text(names_file(twins, lowers))
        stubs = root / "stubs"
        stubs.mkdir()
        calls = root / "cargo-calls"
        (stubs / "cargo").write_text(f'#!/bin/sh\necho "cargo $*" >> "{calls}"\nexit 97\n')
        (stubs / "cargo").chmod(0o755)
        env = {**os.environ, "PATH": f"{stubs}:{os.environ['PATH']}"}
        r = subprocess.run(["bash", str(SCRIPT), "none"], cwd=root, env=env, capture_output=True, text=True)
        assert not calls.exists(), f"{label}: the name check called cargo: {calls.read_text()}"
    fails = [line for line in r.stdout.splitlines() if line.startswith("FAIL")]
    got_missing = set().union(*(named(f, MISSING) for f in fails if MISSING in f))
    got_unknown = set().union(*(named(f, UNKNOWN) for f in fails if UNKNOWN in f))
    other = [f for f in fails if MISSING not in f and UNKNOWN not in f]
    want_exit = 1 if want_missing or want_unknown else 0
    problems = []
    if got_missing != want_missing:
        problems.append(f"(a) missing names {sorted(got_missing)}, want {sorted(want_missing)}")
    if got_unknown != want_unknown:
        problems.append(f"(b) unknown names {sorted(got_unknown)}, want {sorted(want_unknown)}")
    if r.returncode != want_exit or other:
        problems.append(f"(c) exit {r.returncode}, want {want_exit}; other FAIL lines {other}")
    if problems:
        return [f"{label}: twins={twins} lowers={lowers} table={table}: {p}\n  stdout: {r.stdout!r}"
                for p in problems]
    return []


def draw_name(rng):
    while True:
        name = "".join(rng.choice(ALPHABET) for _ in range(rng.randint(1, 12)))
        if name != "none":
            return name


def draw(rng):
    pool = set()
    while len(pool) < 14:
        pool.add(draw_name(rng))
    pool = sorted(pool)
    rng.shuffle(pool)
    twins = [(n, rng.random() < 0.3) for n in pool[:rng.randint(1, 6)]]
    lowers = [(n, rng.random() < 0.3) for n in pool[6:6 + rng.randint(1, 6)]]
    held = [n for n, _ in twins + lowers]
    table = [n for n in held if rng.random() < 0.7] + pool[12:12 + rng.randint(0, 2)]
    rng.shuffle(table)
    return twins, lowers, table


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**32)))
    cases = int(os.environ.get("CASES", "40"))
    print(f"SEED={seed} CASES={cases}")
    rng = random.Random(seed)
    failures = []
    for label, (twins, lowers, table) in EXAMPLES.items():
        failures += check(f"example '{label}'", twins, lowers, table)
    for i in range(cases):
        failures += check(f"draw {i}", *draw(rng))
    for f in failures:
        print(f"FAIL {f}")
    if failures:
        sys.exit(1)
    print(f"PASS {len(EXAMPLES)} examples and {cases} draws")


if __name__ == "__main__":
    main()
