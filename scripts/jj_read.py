#!/usr/bin/env python3
"""The jj command a scripts test reads VCS facts with: from `LANE_REPO` when set (a tester copy has no `.jj`),
else from this checkout. It never snapshots the working copy. Paths in its arguments and output are relative to that
repo's root, so it runs there.

Run: `python3 scripts/jj_read.py <jj args>` runs jj with that prefix (for shell tests).
"""
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def repo():
    found = os.environ.get("LANE_REPO") or (ROOT if (ROOT / ".jj").is_dir() else None)
    if found is None:
        raise RuntimeError(f"no VCS to read: LANE_REPO is unset and {ROOT} holds no .jj")
    return Path(found)


def prefix():
    return ["jj", "-R", str(repo()), "--ignore-working-copy"]


def jj(*args):
    """jj's stdout; its stderr goes to ours, a non-zero exit raises."""
    return subprocess.run([*prefix(), *args], cwd=repo(), check=True, stdout=subprocess.PIPE, text=True).stdout


def tree_files(root):
    """The files of the tree at `root`, relative to it: the tracked files of its jj working copy, else (a tester copy
    holds exactly the tree under test) every file on disk. `LANE_REPO`'s working copy is another tree."""
    if (root / ".jj").is_dir():
        return subprocess.run(["jj", "file", "list"], cwd=root, check=True, stdout=subprocess.PIPE, text=True).stdout.splitlines()
    return [str(p.relative_to(root)) for p in root.rglob("*") if p.is_file() or p.is_symlink()]


if __name__ == "__main__":
    os.chdir(repo())
    os.execvp("jj", [*prefix(), *sys.argv[1:]])
