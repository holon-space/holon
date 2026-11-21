#!/usr/bin/env python3
"""Red when a kind the keystone run claims a self-attesting suite covers was attested by no gate step.

Usage: check-gate-attestations.py <dir>   <dir>/claimed/<kind> holds where the kind is reported,
                                          <dir>/attested/<kind> is written by the suite's own run.
Exit: 0 nothing missing (or a selected gate, which does not judge), 1 a claimed kind is not attested,
2 the claim record is empty or absent, 3 an unreadable dir or record.
"""
import os
import sys
from pathlib import Path


def missing(claimed, attested):
    return sorted(set(claimed) - set(attested))


def is_selected_run(env):
    return env.get("GATE_SELECT") == "1" and env.get("GATE_FULL") != "1"


def names(path):
    return [p.name for p in path.iterdir()] if path.is_dir() else []


def main(argv, env):
    if is_selected_run(env):
        print("attestations not judged: selected run")
        return 0
    root = Path(argv[1])
    try:
        claimed = names(root / "claimed")
        if not claimed:
            print(f"no claim record under {root / 'claimed'}: the keystone run did not write its claims")
            return 2
        attested = names(root / "attested")
        where = {k: (root / "claimed" / k).read_text().strip() for k in claimed}
    except OSError as e:
        print(f"unreadable attestation record: {e.filename or root}: {e.strerror}")
        return 3
    gone = missing(claimed, attested)
    for kind in gone:
        print(f"claimed kind {kind} was attested by no gate step (reported at: {where[kind]})")
    return 1 if gone else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv, os.environ))
