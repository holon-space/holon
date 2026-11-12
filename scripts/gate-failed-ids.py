#!/usr/bin/env python3
"""The failed test ids of one red step of the integrate gate, one per line, as a rerun runs them again.

    gate-failed-ids.py extra-nextest <junit.xml> <file made before the run>   `<binary-id> <test path>`
    gate-failed-ids.py hand-authored <step log>                              `<case name>`
    gate-failed-ids.py keystone-tour <step log>                              `holon-integration-tests::integration <test path>`
    gate-failed-ids.py keystone-smoke <step log> <shape case name>           `test <test path>` / `case <case name>`

Prints nothing when the red has a cause it cannot name as a test: a list of ids is complete or empty.
Exits 1 on an id outside its grammar, or on a libtest result whose failures block names another count.
"""
from __future__ import annotations

import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

TOUR_BINARY = "holon-integration-tests::integration"
REPLAY = "hand_authored_regressions::hand_authored_keystone_regressions"
WORD = re.compile(r"[^\s(),]+")
CASE = re.compile(r"[a-z0-9-]+")


def word(s: str) -> str:
    assert WORD.fullmatch(s), f"not a test id part: {s!r}"
    return s


def case(s: str) -> str:
    assert CASE.fullmatch(s), f"not a hand-authored case name: {s!r}"
    return s


def extra_nextest(junit: Path, mark: Path) -> list[str]:
    # nextest writes the file at the end of a run, so a run that died leaves the file of an earlier one.
    if not junit.is_file() or junit.stat().st_mtime_ns <= mark.stat().st_mtime_ns:
        return []
    root = ET.parse(junit).getroot()
    red = [c for c in root.iter("testcase") if c.find("failure") is not None or c.find("error") is not None]
    if not red or len(red) != int(root.get("failures", 0)) + int(root.get("errors", 0)):
        return []
    return [f"{word(c.get('classname'))} {word(c.get('name'))}" for c in red]


def hand_authored(log: str) -> list[str]:
    shards = re.findall(r"^HAND-AUTHORED RED: shard (\d+) exit \d+, red cases: (.*), log: \S+$", log, re.M)
    if not shards or any(s.startswith("none") for _, s in shards):
        return []
    # A replay killed by a signal names only the cases before the one that killed it, and prints no result.
    parts = re.split(r"^== hand-authored shard (\d+) \(exit \d+\): \S+ ==$", log, flags=re.M)
    output = dict(zip(parts[1::2], parts[2::2]))
    if any(libtest_failed(output.get(k, "")) != [REPLAY] for k, _ in shards):
        return []
    return sorted({case(c) for _, s in shards for c in s.split(",")})


def libtest_failed(log: str) -> list[str]:
    """The failed tests of every libtest run in the log that printed its result.

    A red result lists its failed tests in the last `failures:` block before it; a binary killed before its
    result names none."""
    names, block = [], []
    for line in log.splitlines():
        if line == "failures:":
            block = []
        elif m := re.fullmatch(r"    (\S+)", line):
            block.append(m[1])
        elif m := re.match(r"test result: (ok|FAILED)\. \d+ passed; (\d+) failed;", line):
            failed = int(m[2])
            assert not failed or len(block) == failed, f"libtest result names {failed} failed tests, its failures block {block}"
            names += block if failed else []
            block = []
    return names


def keystone_tour(log: str) -> list[str]:
    return [f"{TOUR_BINARY} {word(n)}" for n in libtest_failed(log)]


def keystone_smoke(log: str, shape: str) -> list[str]:
    failed = libtest_failed(log)
    if not failed:
        return []
    tests = [f"test {word(n)}" for n in failed if n != REPLAY]
    if REPLAY in failed:
        red = re.findall(r'\] RED case "([^"]*)"', log)
        return tests + [f"case {case(shape)}"] if red == [shape] else []
    # The recipe stops at the first red run, so a red PBT leaves the shape case unrun.
    return tests + [f"case {case(shape)}"]


def main() -> None:
    kind, path, *rest = sys.argv[1:]
    if kind == "extra-nextest":
        (mark,) = rest
        ids = extra_nextest(Path(path), Path(mark))
    else:
        log = Path(path).read_text(errors="replace")
        ids = {"hand-authored": hand_authored, "keystone-tour": keystone_tour, "keystone-smoke": keystone_smoke}[kind](log, *rest)
    if ids:
        print("\n".join(ids))


if __name__ == "__main__":
    main()
