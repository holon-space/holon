#!/usr/bin/env python3
"""Fail when a committed red log cannot be traced back to its tree or re-run.

Every file under docs/Testing/red-logs/:

- starts with a `BASE: <commit>` line and a `TREE: <commit>` line;
- cites no `target/` path. `target/` is git-ignored and purged with the
  workspace, so a red log cites the seed and the command instead. Cargo's own
  artifact paths (`target/{debug,release}/{deps,build}/`) are
  recorded output, not citations, and are allowed.

  check-red-logs.py                 check docs/Testing/red-logs/
  check-red-logs.py --self-check    prove each rule fails a bad log
"""
import re
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RED_LOGS = REPO / "docs/Testing/red-logs"

# shortcut: only the debug and release profile dirs count as cargo output; add a profile here when a red log records its build.
TARGET_CITATION = re.compile(r"(?<![\w-])target/(?!(?:debug|release)/(?:deps|build)/)")
HEADER = (re.compile(r"BASE: [0-9a-f]{7,40}"), re.compile(r"TREE: [0-9a-f]{7,40}"))

_BASE_ONLY = "the red ran on a named base plus uncommitted edits, so no commit holds the red tree"
_NO_COMMIT = "the log names no commit, and the commit that added it holds the fixed tree, not the red one"
HEADER_EXEMPT = {
    "2026-08-04-tracked-layout-neutrality-BASE-AB.txt": "the A/B base was rebuilt from `git show main:<path>` over a dirty tree; no commit holds it",
    "2026-08-04-tracked-layout-neutrality-RED.txt": _NO_COMMIT,
    "2026-08-14-flamegraph-ancestry-RED.txt": _NO_COMMIT,
    "2026-10-10-db-open-corrupt-wal-boot-hint-RED.txt": _BASE_ONLY,
    "2026-10-10-db-open-moves-live-db-aside-RED.txt": _BASE_ONLY,
    "2026-10-10-db-open-sidecar-wal-encoding-lock-RED.txt": _BASE_ONLY,
    "2026-10-10-db-open-signature-leg-RED.txt": _BASE_ONLY,
    "2026-10-10-guard-resolve-fails-open-RED.txt": _BASE_ONLY,
    "2026-10-10-guard-resolve-fails-open-round2-RED.txt": _BASE_ONLY,
    "2026-10-10-guard-unbound-in-app-container-RED.txt": _BASE_ONLY,
    "2026-10-11-guard-presence-scope-pin-RED.txt": _BASE_ONLY,
    "2026-10-11-split-condition-interpolation-precedence-RED.txt": _BASE_ONLY,
    "2026-10-11-split-condition-quotes-parens-RED.txt": _NO_COMMIT,
    "2026-10-12-capmap-skips-provided-methods-RED.txt": _NO_COMMIT,
    "2026-10-12-displayed-text-widget-keyword-surface-RED.txt": _BASE_ONLY,
    "2026-10-12-layered-field-admission-RED.txt": _NO_COMMIT,
    "2026-10-12-layered-field-fallback-tagged-override-RED.txt": _NO_COMMIT,
    "2026-10-12-layered-field-override-and-sync-RED.txt": _NO_COMMIT,
    "2026-10-12-layered-field-own-page-every-field-RED.txt": _NO_COMMIT,
    "2026-10-12-layered-field-own-page-whole-file-RED.txt": _NO_COMMIT,
    "2026-10-12-layered-field-plain-rule-RED.txt": _BASE_ONLY,
    "2026-10-12-layered-field-sibling-order-RED.txt": _NO_COMMIT,
    "2026-10-12-layered-field-value-blind-RED.txt": _NO_COMMIT,
    "2026-10-12-needs-auth-restart-RED.txt": _NO_COMMIT,
}


def problems(name, text, header_exempt):
    found = []
    lines = text.splitlines()
    head = lines[:2] + [""] * (2 - len(lines[:2]))
    headed = all(rule.fullmatch(line) for rule, line in zip(HEADER, head))
    if name in header_exempt:
        if headed:
            found.append(f"{name}: has both header lines, drop its exemption")
    else:
        for lineno, (rule, line, want) in enumerate(zip(HEADER, head, ("BASE", "TREE")), 1):
            if not rule.fullmatch(line):
                found.append(f"{name}: line {lineno} must be `{want}: <commit>`, is {line!r}")
    for lineno, line in enumerate(lines, 1):
        if TARGET_CITATION.search(line):
            found.append(f"{name}:{lineno}: cites a target/ path, cite the seed and command: {line.strip()}")
    return found


def lint(directory, header_exempt):
    names = {p.name for p in directory.iterdir() if p.is_file()}
    found = [f"{name}: header exemption names no red log" for name in sorted(header_exempt.keys() - names)]
    for name in sorted(names):
        found += problems(name, (directory / name).read_text(), header_exempt)
    return found


def self_check():
    header = "BASE: e213097550e6\nTREE: 627b5fff5da2\n"
    body = (
        "Command: PROPTEST_CASES=1 PROPTEST_RNG_SEED=2 cargo nextest run -p p --test t\n"
        "     Running tests/t.rs (target/debug/deps/t-0123456789abcdef)\n"
    )
    cases = {
        "a headed log citing a seed and a command": (header + body, {}, 0),
        "a log citing a target/ path": (header + body + "full log: target/probe/red.log\n", {}, 1),
        "a log with no TREE line": ("BASE: e213097550e6\n" + body, {}, 1),
        "a log citing a lane log under a build/ dir": (header + body + "full log: target/probe/build/red.log\n", {}, 1),
        "an exempt log with no header": (body, {"red.txt": "reason"}, 0),
        "an exempt log that has the header": (header + body, {"red.txt": "reason"}, 1),
    }
    wrong = []
    for name, (text, exempt, want) in cases.items():
        with tempfile.TemporaryDirectory() as tmp:
            (Path(tmp) / "red.txt").write_text(text)
            got = len(lint(Path(tmp), exempt))
        print(f"[red-logs] self-check {name}: {got} failure(s), expected {want}")
        if got != want:
            wrong.append(name)
    return wrong


def main():
    if sys.argv[1:] == ["--self-check"]:
        wrong = self_check()
        if wrong:
            print(f"[red-logs] SELF-CHECK FAIL: {', '.join(wrong)}")
            return 1
        print("[red-logs] self-check PASS")
        return 0
    assert not sys.argv[1:], f"usage: {sys.argv[0]} [--self-check]"
    found = lint(RED_LOGS, HEADER_EXEMPT)
    for line in found:
        print(f"[red-logs] FAIL {line}")
    if found:
        return 1
    print(f"[red-logs] PASS: every red log in {RED_LOGS.relative_to(REPO)} is headed and cites no target/ path")
    return 0


if __name__ == "__main__":
    sys.exit(main())
