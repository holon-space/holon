#!/usr/bin/env python3
"""Every known-red site that pins lines of a file in this repository names a line of the current file that can
panic. A row whose pinned lines all moved classifies its own red as a cross-family match.

Run from the repository root: `python3 scripts/flow-checks/test-known-red-sites.py`.
The sites are the ones the classifier compares: each `known-red` row's Site and the site of each collateral pair,
read through check-known-red-evidence.py, the classifier's own row reader.
Generators: none — the input is the registry itself, and the check reads every row of it.
"""
import importlib.util
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
REGISTRY = REPO / "docs/Testing/KeystoneKnownReds.md"
spec = importlib.util.spec_from_file_location("evidence", REPO / "scripts/check-known-red-evidence.py")
evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evidence)

# A macro invocation line is the site of the tests it expands to.
PANICS = re.compile(r"assert|panic!|unwrap\(|expect\(|unreachable!|todo!|unimplemented!|^\s*[a-z_:]+!\s*\{\s*$")
FILES = sorted(f for f in evidence.landing_paths() if f.endswith(".rs"))
assert FILES, "the landing tree carries no Rust file"


def sites():
    for row in evidence.registry_rows(REGISTRY):
        assert len(row) == 8, f"registry row the grammar refuses: {row}"
        _, key, status, _, site, _, _, collateral = row
        if status != "known-red":
            continue
        for s in [site] + [pair_site for _, pair_site in collateral]:
            if s:
                yield key, s


def pinned_lines(site, path, lines):
    """The line numbers of `path` the site ERE accepts, or None when it accepts any line."""
    ere = re.compile(site, re.M)
    if ere.search(f"{path}:{len(lines) + 7}:1"):
        return None
    # One candidate location per line of the text, so `^`/`$` anchor each.
    return [n for n, text in enumerate(lines, 1)
            if ere.search("\n".join(f"{path}:{n}:{col}" for col in range(1, len(text) + 2)))]


failures = []
checked = 0
for key, site in sites():
    literal = site.replace("\\", "")
    for path in FILES:
        if f"{Path(path).parent.name}/{Path(path).name}" not in literal:
            continue
        lines = (REPO / path).read_text().splitlines()
        numbers = pinned_lines(site, path, lines)
        literals = [int(d) for d in re.findall(r"[0-9]+", site)]
        past_end = sorted({n for n in literals if n > len(lines)
                           for col in literals if re.search(site, f"{path}:{n}:{col}")})
        if past_end:
            checked += 1
            failures.append(f"{key}: {path} pins line(s) {past_end} past its last line {len(lines)}")
            continue
        if not numbers:
            continue
        checked += 1
        if not any(PANICS.search(lines[n - 1]) for n in numbers):
            failures.append(f"{key}: {path}:{numbers} holds no panic site: "
                            + "; ".join(f"{n}: {lines[n - 1].strip()!r}" for n in numbers))
assert checked > 0, "no known-red site pins a line of a file in this repository"
print(f"known-red sites pinning repository lines checked: {checked}")
if failures:
    print("\n".join(failures))
    sys.exit(1)
print("every pinned site holds a panic site")
