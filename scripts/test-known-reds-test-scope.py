#!/usr/bin/env python3
"""Self-check of scripts/keystone-known-reds.sh: a known-red row that names tests
(`; tests `<unit>, <unit>`` in its Companions cell) covers its panic in exactly
those tests; the same panic in any other test, or where no test proves its owner,
is novel. A row that names no test covers every test, as before.

Fixed cases, no property test: the input is the shape of one log (one panic printed
under one test name) and the verdict is one of two words, so each case is the whole
class; the generated-log checks (test-known-reds-classifier.py) cover the rest.

Run: `python3 scripts/test-known-reds-test-scope.py` (CLASSIFIER=<script> names the classifier under test).
"""
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

import known_reds_logs as logs

CLASSIFIER = Path(os.environ.get("CLASSIFIER", Path(__file__).resolve().parent / "keystone-known-reds.sh")).resolve()
REPO = CLASSIFIER.parent.parent
NAMED = "holon::main scope::the_named_test"
OTHER = "holon::main scope::the_other_test"
PATTERN = "^scoped defect [0-9]+$"
SITE = "^crates/x\\.rs:[0-9]+:[0-9]+$"
MESSAGE = "scoped defect 7"


def row(tests):
    scope = f"—; tests `{', '.join(tests)}`" if tests else "—"
    return f"| `scoped-row` | known-red | `{PATTERN}` | `{SITE}` | sig | evidence | #0 | #0 | {scope} |\n"


def captured(*units):
    """A nextest run in which each unit fails with MESSAGE."""
    blocks = [f"        FAIL [   1.000s] ({i}/{len(units)}) {u}\n  stderr ───\n\n"
              f"    thread '{u.split()[-1]}' (1) panicked at crates/x.rs:1:1:\n    {MESSAGE}\n\n"
              for i, u in enumerate(units, 1)]
    return "".join(blocks) + f"     Summary [   2.000s] {len(units)} tests run: 0 passed, {len(units)} failed, 0 skipped\n"


def libtest(name):
    """A libtest section, whose panic text names no nextest unit."""
    return ("     Running tests/main.rs (target/debug/deps/x-0123)\n\nrunning 1 test\n"
            f"thread '{name}' (5) panicked at crates/x.rs:1:1:\n{MESSAGE}\n"
            f"test {name} ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s\n")


def classify(work, tests, text):
    tree = Path(tempfile.mkdtemp(dir=work))
    (tree / "docs/Testing").mkdir(parents=True)
    (tree / "scripts").symlink_to(CLASSIFIER.parent)
    (tree / "docs/Testing/known-red-evidence").symlink_to(REPO / "docs/Testing/known-red-evidence")
    registry = (REPO / "docs/Testing/KeystoneKnownReds.md").read_text()
    (tree / "docs/Testing/KeystoneKnownReds.md").write_text(registry.replace(logs.REGISTRY_HEADER, logs.REGISTRY_HEADER + row(tests), 1))
    log = tree / "case.log"
    log.write_text(text)
    r = subprocess.run([str(tree / "scripts" / CLASSIFIER.name), str(log)], capture_output=True, text=True)
    return r.returncode, r.stdout + r.stderr


CASES = [
    ("named test, panic in it", [NAMED], captured(NAMED), 0, r"PASS-WITH-NOTE: 1 known-red panic\(s\), 0 novel"),
    ("named test, panic in another", [NAMED], captured(OTHER), 1, r"FAIL: 1 novel panic\(s\), 0 known-red"),
    ("named and another test, same panic", [NAMED], captured(NAMED, OTHER), 1, r"FAIL: 1 novel panic\(s\), 1 known-red"),
    ("two named tests, panic in the second", [NAMED, OTHER], captured(OTHER), 0, r"PASS-WITH-NOTE: 1 known-red panic\(s\), 0 novel"),
    ("name that only starts with the named test", [NAMED], captured(NAMED + "_more"), 1, r"FAIL: 1 novel panic\(s\)"),
    ("name that only ends with the named test", [NAMED], captured("holon::main outer::" + NAMED.split()[-1]), 1, r"FAIL: 1 novel panic\(s\)"),
    ("named binary differs", [NAMED], captured(NAMED.replace("holon::main", "holon::other")), 1, r"FAIL: 1 novel panic\(s\)"),
    ("libtest section, no proven owner", [NAMED], libtest(NAMED.split()[-1]), 1, r"FAIL: 1 novel panic\(s\)"),
    ("row without tests, panic in any test", [], captured(OTHER), 0, r"PASS-WITH-NOTE: 1 known-red panic\(s\), 0 novel"),
    ("row without tests, libtest section", [], libtest(OTHER.split()[-1]), 0, r"PASS-WITH-NOTE: 1 known-red panic\(s\), 0 novel"),
]


def main():
    failed = 0
    with tempfile.TemporaryDirectory() as tmp:
        for name, tests, text, want_exit, want_line in CASES:
            code, out = classify(Path(tmp), tests, text)
            ok = code == want_exit and re.search(want_line, out)
            print(f"[test-scope] {'ok  ' if ok else 'FAIL'} {name}: exit {code}, want {want_exit} and /{want_line}/")
            if not ok:
                failed += 1
                print(out)
    print(f"[test-scope] {'FAIL' if failed else 'PASS'}: {failed} of {len(CASES)} case(s)")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
