#!/usr/bin/env python3
"""Property check of `just cargo-free-checks`, the gate checks that build nothing.

Run: `python3 scripts/test-cargo-free-checks.py` (CASES=<n> draws n rows, default 2).
It works in a copy of this tree whose `cargo` is a stub that records each call:
- the unchanged tree passes the recipe, and the recipe calls no cargo;
- a known-red row whose slug FeatureMap.md does not list fails the recipe and
  `just gate-arch`, both before any cargo call, with the feature-map drift named.
Exits non-zero when a property fails; each failing row is printed as an EXAMPLES entry.
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
REGISTRY = Path("docs/Testing/KeystoneKnownReds.md")
COMPANIONS_HEADER = re.compile(r"^\| Key \|.*\| Companions \|$")
DRIFT = "docs/Architecture/FeatureMap.md has drifted"
SLUG_HEAD = "abcdefghijklmnopqrstuvwxyz"
SLUG_TAIL = SLUG_HEAD + "0123456789-"
# (slug, Companions table index, row offset in that table), run before the drawn cases.
EXAMPLES = [("a-new-known-red", 0, 0)]


def copy_tree(dest):
    """The files of this tree's commit, in a git top level of their own (check-known-red-evidence.py asks git)."""
    files = subprocess.run(["jj", "file", "list"], cwd=REPO, check=True, capture_output=True, text=True).stdout.split("\n")
    for rel in filter(None, files):
        (dest / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(REPO / rel, dest / rel, follow_symlinks=False)
    subprocess.run(["git", "init", "-q"], cwd=dest, check=True)


def cargo_stub(stubs, calls):
    stubs.mkdir()
    cargo = stubs / "cargo"
    cargo.write_text(f'#!/bin/sh\necho "cargo $*" >> "{calls}"\nexit 97\n')
    cargo.chmod(0o755)


def run(tree, env, calls, recipe):
    """(exit code, combined output, the cargo calls) of `just <recipe>` in `tree`."""
    calls.write_text("")
    r = subprocess.run(["just", recipe], cwd=tree, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    return r.returncode, r.stdout, calls.read_text().splitlines()


def tail(out):
    return "\n".join("  | " + line for line in out.splitlines()[-40:])


def tables(lines):
    """(first row index, row count) of every registry table with a Companions column."""
    found = []
    for i, line in enumerate(lines):
        if COMPANIONS_HEADER.match(line):
            first = i + 2
            end = first
            while end < len(lines) and lines[end].startswith("|"):
                end += 1
            found.append((first, end - first))
    assert found, f"{REGISTRY}: no table with a Companions column"
    return found


def known_keys(lines):
    return {m.group(1) for line in lines if (m := re.match(r"^\|\s*`([^`]+)`", line))}


def draw(rng, lines):
    # A slug that is already a key is not a missing one: the map may already list it.
    keys = known_keys(lines)
    while True:
        slug = rng.choice(SLUG_HEAD) + "".join(rng.choice(SLUG_TAIL) for _ in range(rng.randrange(40)))
        if slug not in keys:
            break
    table = rng.randrange(len(tables(lines)))
    return slug, table, rng.randrange(tables(lines)[table][1] + 1)


def with_row(lines, slug, table, offset):
    """The registry with one lint-clean known-red row for `slug` at `offset` of Companions table `table`."""
    first, count = tables(lines)[table]
    assert offset <= count, f"table {table} has {count} rows, offset {offset}"
    row = f"| `{slug}` | known-red | `{slug}-signature` | — | a generated red | — | — | — | — |"
    return lines[: first + offset] + [row] + lines[first + offset :]


def main():
    cases = int(os.environ.get("CASES", "2"))
    seed = random.randrange(2**32)
    rng = random.Random(seed)
    failures = []
    with tempfile.TemporaryDirectory() as scratch:
        scratch = Path(scratch)
        tree, calls = scratch / "tree", scratch / "cargo-calls"
        copy_tree(tree)
        cargo_stub(scratch / "stubs", calls)
        env = {**os.environ, "PATH": f"{scratch / 'stubs'}:{os.environ['PATH']}"}

        rc, out, called = run(tree, env, calls, "cargo-free-checks")
        print(f"unchanged tree: exit {rc}, cargo calls {called}")
        if rc != 0:
            failures.append(f"the unchanged tree fails the recipe:\n{tail(out)}")
        if called:
            failures.append(f"the recipe calls cargo: {called}")

        registry = tree / REGISTRY
        original = registry.read_text()
        lines = original.splitlines()
        drawn = [draw(rng, lines) for _ in range(cases)]
        print(f"drawn with seed {seed}: {drawn}")
        for case in EXAMPLES + drawn:
            registry.write_text("\n".join(with_row(lines, *case)) + "\n")
            for recipe in ("cargo-free-checks", "gate-arch"):
                rc, out, called = run(tree, env, calls, recipe)
                print(f"{case}: just {recipe}: exit {rc}, cargo calls {called}")
                if rc == 0 or called or DRIFT not in out:
                    failures.append(f"EXAMPLES entry {case}: `just {recipe}` exit {rc}, cargo calls {called}, "
                                    f"drift named: {DRIFT in out}\n{tail(out)}")
            registry.write_text(original)
    for failure in failures:
        print(f"FAIL: {failure}")
    print(f"test-cargo-free-checks: {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
