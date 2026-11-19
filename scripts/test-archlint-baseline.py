#!/usr/bin/env python3
"""Properties of `archlint --all` against its baseline, over drawn scans and drawn baselines.

Run: `python3 scripts/test-archlint-baseline.py` (CASES=<n>, SEED=<s>). Needs Python >= 3.11 (archlint reads TOML).
The scan is replaced by a drawn multiset of violation identities and BASELINE_FILE points at a temp file holding a drawn
multiset, so the properties hold for any rule set. For each draw, in text and in JSON mode:
- the exit is non-zero iff a scanned identity is missing from the baseline (new) or a baselined one is not scanned (stale);
- the stale identities the run prints, one per line, equal the baseline minus the scan as a multiset, and the printed
  lines are valid baseline entries (`load_baseline` reads them back to the same multiset);
- text and JSON mode agree on the exit and on the printed stale lines.
Generators: the scan and the baseline each give every identity of a six-identity pool a count of 0-3; the fixed examples
(one stale entry alone, a baselined count of 2 against 1 firing, an empty baseline, an empty scan against a non-empty
baseline) run first through the same properties.
"""
import contextlib
import importlib.util
import io
import os
import random
import sys
import tempfile
from collections import Counter
from pathlib import Path

assert sys.version_info >= (3, 11), f"archlint needs Python >= 3.11, running {sys.version}"

ARCHLINT = Path(__file__).resolve().parent.parent / "archlint/archlint.py"
POOL = [f"{rule}\t{path}\t{line}" for rule, path, line in [
    ("no-unwrap", "crates/a/src/lib.rs", "let x = y.unwrap();"),
    ("no-unwrap", "crates/b/src/lib.rs", "let x = y.unwrap();"),
    ("no-as-str-match", "crates/a/src/lib.rs", "match s.as_str() {"),
    ("no-catch-unwind", "crates/c/src/main.rs", "std::panic::catch_unwind(f)"),
    ("handoff-root", "HANDOFF.md", ""),
    ("no-unwrap", "crates/a/src/lib.rs", "let z = w.unwrap();"),
]]

FIXED = {
    "one stale entry alone": ({}, {POOL[0]: 1}),
    "two baselined, one firing": ({POOL[0]: 1}, {POOL[0]: 2}),
    "one new entry beside a clean baselined one": ({POOL[0]: 1, POOL[1]: 1}, {POOL[0]: 1}),
    "an empty baseline": ({POOL[2]: 2}, {}),
    "an empty scan against a non-empty baseline": ({}, {POOL[3]: 1, POOL[4]: 3}),
    "scan and baseline equal": ({POOL[0]: 2, POOL[5]: 1}, {POOL[0]: 2, POOL[5]: 1}),
    "both empty": ({}, {}),
}


def load_archlint():
    spec = importlib.util.spec_from_file_location("archlint_under_test", ARCHLINT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run(scan, baseline, fmt):
    """Returns (exit, stderr stale lines) of `cmd_all(fmt)`; a stale line is the only kind of stderr line with a tab."""
    arch = load_archlint()
    diags = [{"id": ident.split("\t")[0], "file": ident.split("\t")[1], "line": 1, "message": "m", "ident": ident}
             for ident, n in scan.items() for _ in range(n)]
    with tempfile.TemporaryDirectory() as d:
        arch.BASELINE_FILE = Path(d) / "baseline.txt"
        arch.BASELINE_FILE.write_text("# header\n\n" + "".join(f"{i}\n" for i, n in baseline.items() for _ in range(n)))
        arch.scan_all = lambda: (diags, len(diags))
        arch.diag_identity = lambda diag: diag["ident"]
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = arch.cmd_all(fmt)
        printed = [line for line in err.getvalue().splitlines() if "\t" in line]
        reread = Path(d) / "printed.txt"
        reread.write_text("\n".join(printed) + "\n")
        arch.BASELINE_FILE = reread
        assert arch.load_baseline() == Counter(printed), (printed, "a printed stale line is not a baseline entry")
    return code, printed


def check(name, scan, baseline):
    scan_c, base_c = Counter(scan), Counter(baseline)
    new, stale = scan_c - base_c, base_c - scan_c
    want_code = 2 if new or stale else 0
    results = {fmt: run(scan, baseline, fmt) for fmt in ("text", "json")}
    for fmt, (code, printed) in results.items():
        assert code == want_code, (name, fmt, "exit", code, want_code)
        assert Counter(printed) == stale, (name, fmt, "stale lines", sorted(printed), sorted(stale.elements()))
    assert results["text"] == results["json"], (name, results)
    return bool(stale)


def main():
    rng = random.Random(int(os.environ.get("SEED", 0)))
    for name, (scan, baseline) in FIXED.items():
        check(name, scan, baseline)
        print(f"[archlint baseline] {name}: PASS")
    cases, stale_cases = int(os.environ.get("CASES", 60)), 0
    for case in range(cases):
        scan = {i: rng.randint(0, 3) for i in POOL}
        baseline = {i: rng.randint(0, 3) for i in POOL}
        stale_cases += check(f"case {case}", scan, baseline)
    assert 0 < stale_cases < cases, f"{stale_cases} of {cases} drawn runs had a stale entry"
    print(f"[archlint baseline] {cases} drawn runs PASS ({stale_cases} with a stale entry)")


if __name__ == "__main__":
    main()
