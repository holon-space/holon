#!/usr/bin/env python3
"""Property check of the hand-authored row-key rule of scripts/check-known-red-evidence.py, run as gate-arch runs it.

Run: `python3 scripts/test-known-red-evidence-row-key.py` (SEED=<n>, CASES=<n>, default 30).
Each case writes a registry and a hand-authored case file into a temp copy of the checker's tree and runs the
checker with no argument there:
- the run exits 1 exactly when a case's `known_red` names a key with no `known-red` row (missing, or a row of
  another status), and each such case prints one line naming its file, line and key;
- a `known_red` value that is not a string fails the run with its file and line.
Fixed cases run first. Each failing case is printed with its rows and cases.
"""
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "scripts/flow-checks"))
import known_reds_table  # noqa: E402

CASE_DIR = "crates/holon-integration-tests/hand-authored-regressions"
CASE_FILE = f"{CASE_DIR}/cases.jsonl"
STATUSES = known_reds_table.STATUSES
COMMENT = "# Hand-authored cases."


def one_file(cases):
    """{case file: its lines}, a line being a case (name, known_red or None) or a raw comment or blank line."""
    return {"cases.jsonl": [COMMENT, "", *cases]}


# (registry rows {key: status}, case files, expected exit), run before the drawn ones.
EXAMPLES = [
    ({"alpha": "known-red"}, one_file([("names-an-existing-row", "alpha")]), 0),
    ({"alpha": "known-red"}, one_file([("names-a-missing-row", "gone")]), 1),
    ({"alpha": "known-red", "beta": "fixed-pending-soak"}, one_file([("names-a-soaking-row", "beta")]), 1),
    ({"alpha": "known-red"}, one_file([("names-no-row", None), ("names-an-existing-row", "alpha")]), 0),
    ({"alpha": "known-red"}, {"a.jsonl": [("names-an-existing-row", "alpha")], "b.jsonl": ["", ("names-a-missing-row", "gone")]}, 1),
]
# The Rust replay parser panics on each of these too, `null` and the empty key included.
NON_STRING = [3, ["alpha"], {"key": "alpha"}, None, True, ""]
# Case lines that are no case: (label, line).
MALFORMED = [("not JSON", '{"name": "half-written", "known_red": "alpha"'),
             ("a JSON array", '["names-an-existing-row", "alpha"]'),
             ("a case without a name", json.dumps({"description": "d", "known_red": "gone", "transitions": []}))]


def tree(tmp, rows, files):
    for rel in ("scripts/check-known-red-evidence.py", "scripts/flow-checks/known_reds_table.py"):
        (tmp / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(REPO / rel, tmp / rel)
    header = known_reds_table.HEADER
    table = [f"| {' | '.join(header)} |", "|" + " --- |" * len(header)]
    table += [f"| `{k}` | {s} | `{k}-panic` | — | sig | ev | — | — | — |" for k, s in rows.items()]
    registry = tmp / "docs/Testing/KeystoneKnownReds.md"
    registry.parent.mkdir(parents=True, exist_ok=True)
    registry.write_text("# Registry\n\n" + "\n".join(table) + "\n")
    # The checker fails a tree without a run record file; a header-only one holds no record to resolve.
    runs = tmp / "docs/Testing/known-red-evidence/k/gate-load-runs.tsv"
    runs.parent.mkdir(parents=True)
    runs.write_text("test\tjob\tstarted\tlog\n")
    (tmp / CASE_DIR).mkdir(parents=True)
    for name, lines in files.items():
        (tmp / CASE_DIR / name).write_text("".join((line if isinstance(line, str) else case_line(*line)) + "\n"
                                                   for line in lines))


def case_line(name, known_red):
    case = {"name": name, "description": "d", "transitions": []}
    if known_red is not None:
        case["known_red"] = known_red
    return json.dumps(case)


def run(rows, files):
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        tree(tmp, rows, files)
        r = subprocess.run([sys.executable, str(tmp / "scripts/check-known-red-evidence.py")], cwd=tmp,
                           stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    return r.returncode, r.stdout


def key_problems(rows, files, want=None):
    bad = [(f"{CASE_DIR}/{f}:{i}", line[0], line[1]) for f, lines in files.items() for i, line in enumerate(lines, 1)
           if not isinstance(line, str) and line[1] is not None and rows.get(line[1]) != "known-red"]
    assert want is None or want == (1 if bad else 0), (rows, files, want)
    code, out = run(rows, files)
    found = []
    if code != (1 if bad else 0):
        found.append(f"exit {code}, expected {1 if bad else 0}: {out.strip()!r}")
    for where, name, key in bad:
        if not any(f"{where}:" in o and name in o and key in o for o in out.splitlines()):
            found.append(f"no output line names {where}, case {name} and row {key}: {out.strip()!r}")
    named = [o for o in out.splitlines() if CASE_DIR in o]
    if len(named) != len(bad):
        found.append(f"{len(named)} output lines name a case file, {len(bad)} cases name no known-red row: {named}")
    return found


def refused_problems(label, line):
    """A case line the Rust parser refuses fails the run with its file and line."""
    code, out = run({"alpha": "known-red"}, one_file([line]))
    if code == 0 or f"{CASE_FILE}:3:" not in out:
        return [f"{label}: exit {code}, expected non-zero naming {CASE_FILE}:3: {out.strip()!r}"]
    return []


def draw(rng):
    keys = [f"row-{i}" for i in range(rng.randint(0, 5))]
    rows = {k: rng.choice(STATUSES) for k in keys}
    known = [k for k, s in rows.items() if s == "known-red"] or [None]
    other = keys + ["missing-row", "other-missing"]
    files = {}
    for f in rng.sample(["a.jsonl", "keystone.jsonl", "z-late.jsonl"], rng.randint(1, 3)):
        lines = []
        for i in range(rng.randint(0, 5)):
            lines += rng.choice([[], [], [""], [COMMENT]])
            lines.append((f"{f}-case-{i}", rng.choice(known if rng.random() < 0.75 else other)))
        files[f] = lines
    return rows, files


def main():
    seed = int(os.environ.get("SEED", random.randrange(2**32)))
    rng = random.Random(seed)
    drawn = [draw(rng) for _ in range(int(os.environ.get("CASES", "30")))]
    print(f"drawn with seed {seed}")
    failures = []
    for rows, files, want in EXAMPLES:
        failures += [f"EXAMPLES entry ({rows}, {files}, {want}): {f}" for f in key_problems(rows, files, want)]
    for value in NON_STRING:
        failures += refused_problems(f"known_red {value!r}", json.dumps(
            {"name": "names-a-non-string", "description": "d", "known_red": value, "transitions": []}))
    for label, line in MALFORMED:
        failures += refused_problems(label, line)
    for rows, files in drawn:
        failures += [f"({rows}, {files}): {f}" for f in key_problems(rows, files)]
    reds = sum(any(not isinstance(c, str) and c[1] is not None and r.get(c[1]) != "known-red"
                   for lines in fs.values() for c in lines) for r, fs in drawn)
    print(f"drawn cases: {len(drawn) - reds} all keys known-red, {reds} with a key that names no known-red row")
    for f in failures:
        print(f"FAIL: {f}")
    print(f"test-known-red-evidence-row-key: {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
