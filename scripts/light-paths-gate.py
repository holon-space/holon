#!/usr/bin/env python3
"""Flow command gate for the move of a `light` card into `verify`: refuse when a linked PR changes a path that
the landing gate selects work for.

Run from the workspace root with the variables flow sets: FLOW_ISSUE, FLOW_TO (must be `verify`) and FLOW_PR_HEADS
(`owner/name#N@sha`, space separated; flow leaves it unset for a card with no linked PR). Only heads of HOLON_REPO are
judged; heads of other repos are named as skipped and never fetched. The PRs' changed files come from the REST pull-request files list, old and new
path of a rename alike; what counts as gate-relevant is decided by `gate_selection` of scripts/select-tests.py: a full
run, or any step outside its TREE_STEPS.

Exit 0: no holon head, or every holon head is gate-free. Any other exit refuses; flow shows only stderr, so the paths, their reasons and the
repair command are printed there.
"""
import contextlib
import importlib.util
import io
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GATE = "integrate"
HOLON_REPO = "holon-space/holon"
HEAD = re.compile(r"^(?P<repo>[^/#@\s]+/[^/#@\s]+)#(?P<number>\d+)@(?P<sha>[0-9a-f]+)$")


def load_select_tests():
    spec = importlib.util.spec_from_file_location("select_tests", ROOT / "scripts/select-tests.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ST = load_select_tests()


@dataclass(frozen=True)
class Head:
    repo: str
    number: int
    sha: str


def parse_heads(text):
    heads = []
    for token in text.split():
        m = HEAD.match(token)
        if not m:
            sys.exit(f"FLOW_PR_HEADS entry {token!r} is not owner/name#N@sha")
        heads.append(Head(m["repo"], int(m["number"]), m["sha"]))
    if not heads:
        sys.exit("no linked PR head: cannot judge a light card")
    return heads


def gh_json(args, environ):
    out = subprocess.run(["gh", "api", *args], check=True, stdout=subprocess.PIPE, text=True, env=dict(environ)).stdout
    return json.loads(out)


def pr_files(head, environ):
    """Old and new path of every changed file of the PR; exits when the list is empty or not the whole PR."""
    pull = f"repos/{head.repo}/pulls/{head.number}"
    files = [f for page in gh_json(["--paginate", "--slurp", f"{pull}/files"], environ) for f in page]
    changed = gh_json([pull], environ)["changed_files"]
    if not files:
        sys.exit(f"{head.repo}#{head.number} lists no changed file")
    if len(files) != changed:
        sys.exit(f"{head.repo}#{head.number}: gh listed {len(files)} of {changed} changed files")
    return sorted({p for f in files for p in (f["filename"], *([f["previous_filename"]] if "previous_filename" in f else []))})


def relevant_steps(paths, meta):
    """[(step, reason)] of the selection of `paths` that is not a tree-wide check; [("*", path)] for a full run."""
    with contextlib.redirect_stderr(io.StringIO()):
        selected = ST.gate_selection(ROOT, meta, GATE, paths)
    return [(step, reason) for step, reason, _ in selected if step == "*" or step not in ST.TREE_STEPS]


def refusal(paths, meta):
    """Lines naming why `paths` are gate-relevant; empty when they are gate-free."""
    together = relevant_steps(paths, meta)
    if not together:
        return []
    lines = []
    for p in paths:
        steps = relevant_steps([p], meta)
        if steps:
            lines.append(f"{p}: " + ", ".join("full run" if step == "*" else step for step, _ in steps))
    return lines or [f"{len(paths)} paths together select {together}"]


def tree_id():
    return subprocess.run(["jj", "log", "--ignore-working-copy", "--no-graph", "-r", "@", "-T", "commit_id"],
                          cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True).stdout


def run(environ):
    if environ["FLOW_TO"] != "verify":
        sys.exit(f"FLOW_TO is {environ['FLOW_TO']!r}: this gate belongs on the move into verify")
    issue = environ["FLOW_ISSUE"]
    if "FLOW_PR_HEADS" not in environ:
        print(f"light card #{issue}: no linked PR, no holon path to judge", file=sys.stderr)
        return 0
    heads = parse_heads(environ["FLOW_PR_HEADS"])
    for h in heads:
        if h.repo != HOLON_REPO:
            print(f"skipped head of {h.repo}, not {HOLON_REPO}: {h.repo}#{h.number}@{h.sha}", file=sys.stderr)
    heads = [h for h in heads if h.repo == HOLON_REPO]
    if not heads:
        print(f"light card #{issue}: no linked PR, no holon path to judge", file=sys.stderr)
        return 0
    paths = sorted({p for h in heads for p in pr_files(h, environ)})
    meta = ST.load_metadata()
    ST.assert_workspace_root(meta, ROOT)
    lines = refusal(paths, meta)
    print(f"TREE: {tree_id()}", file=sys.stderr)
    if not lines:
        print(f"light card #{issue}: PR heads are gate-free", file=sys.stderr)
        return 0
    print(f"light card #{issue}: its PR changes gate-relevant paths:", *lines, f"repair: flow set-work-type {issue} code",
          sep="\n", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(run(os.environ))
