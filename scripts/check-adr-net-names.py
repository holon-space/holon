#!/usr/bin/env python3
"""Fail when ADR 0032 names a net transition or place that no net source defines.

A whole code span of one PascalCase word (`DiskChanged`, `Base`) in the ADR names
a transition or place, unless EXEMPT lists it. Each such name must equal the
value of a `const NAME: &str = "Value";` in crates/holon-net/ or
crates/holon-filesystem/src/writeback_net.rs. An exemption must still occur in
the ADR and must not resolve, so the list does not outlive its reason.

  check-adr-net-names.py [ROOT]       default: the repo this script lives in
  check-adr-net-names.py --self-check prove each rule fails a bad fixture
"""

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ADR = Path("docs/adr/0032-petri-net-execution-semantics.md")
NET_SOURCES = ["crates/holon-net/**/*.rs", "crates/holon-filesystem/src/writeback_net.rs"]

NAME = re.compile(r"`([A-Z][a-z0-9]+(?:[A-Z][a-z0-9]*)*)`")
CONST = re.compile(r'\bconst\s+[A-Z][A-Z0-9_]*\s*:\s*&str\s*=\s*"([A-Z][a-z0-9]+(?:[A-Z][a-z0-9]*)*)"')

EXEMPT = {
    # Rust items and values, not net names.
    "BoundaryEnforcer", "Definite", "Err", "FreeStanding", "GuardWorld", "Hierarchical",
    "None", "OperationDescriptor", "OperationDispatcher", "Page", "StorageVersion",
    "Transition", "UndoStack", "UndoStore", "Unknown", "WriteSeq",
    # Net-file vocabulary: capacity kinds, delivery modes, settle reasons.
    "Keyed", "OverChangedInputs", "Register", "Single", "StateProbe",
    # Derived states of the write-back net, not places.
    "InSync",
    # Names of nets the ADR designs that no net source defines.
    "AcquireWriterLock", "MakeLoroReady", "MakeTursoReady", "NotWritten", "OpenDeviceStore",
    "OpenTurso", "PromoteStandby", "Recover", "Rekey", "Running", "SecondaryStorage",
    "Storage", "StorageCandidate", "StorageRequest", "SwapStorage", "WriterLock", "Writing",
}


def net_names(root):
    return {m[1] for pattern in NET_SOURCES for f in root.glob(pattern) for m in CONST.finditer(f.read_text())}


def lint(adr_text, consts, exempt):
    failures = []
    seen = set()
    for lineno, line in enumerate(adr_text.splitlines(), 1):
        for m in NAME.finditer(line):
            seen.add(m[1])
            if m[1] not in consts and m[1] not in exempt:
                failures.append(f"{ADR}:{lineno}: `{m[1]}` resolves to no const in the net sources")
    for name in sorted(exempt - seen):
        failures.append(f"EXEMPT `{name}` no longer occurs in {ADR}; drop it")
    for name in sorted(exempt & consts):
        failures.append(f"EXEMPT `{name}` resolves to a net const; drop it")
    return failures


def self_check():
    consts = {m[1] for m in CONST.finditer('pub const CATCH_UP_INGEST: &str = "CatchUpIngest";\nconst BASE: &str = "Base";\nconst SEED: &str = "seed";')}
    cases = {
        "the const values": (len(consts ^ {"CatchUpIngest", "Base"}), 0),
        "a line with only known names": (len(lint("`CatchUpIngest` writes `Base`.", consts, set())), 0),
        "a line with an unknown transition": (len(lint("`CatchUp` inhibits `Base`.", consts, set())), 1),
        "an exempt name": (len(lint("`GuardWorld` reads `Base`.", consts, {"GuardWorld"})), 0),
        "an exemption the ADR no longer names": (len(lint("`Base`.", consts, {"GuardWorld"})), 1),
        "an exemption that resolves": (len(lint("`Base`.", consts, {"Base"})), 1),
    }
    wrong = []
    for name, (got, want) in cases.items():
        print(f"[adr-net-names] self-check {name}: {got} failure(s), expected {want}")
        if got != want:
            wrong.append(name)
    return wrong


def main(argv):
    if argv == ["--self-check"]:
        wrong = self_check()
        if wrong:
            print(f"[adr-net-names] SELF-CHECK FAILED: {', '.join(wrong)}", file=sys.stderr)
            return 1
        return 0
    root = Path(argv[0]) if argv else REPO
    consts = net_names(root)
    assert consts, f"no PascalCase net consts under {root}: {NET_SOURCES}"
    failures = lint((root / ADR).read_text(), consts, EXEMPT)
    for failure in failures:
        print(f"[adr-net-names] {failure}", file=sys.stderr)
    if failures:
        print("[adr-net-names] FAIL: rename the ADR name to the net's const value, or exempt a non-net word", file=sys.stderr)
        return 1
    print(f"[adr-net-names] PASS: every net name in {ADR} resolves")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
