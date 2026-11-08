#!/usr/bin/env python3
"""A rebased lockfile keeps every entry of the new base, except where the
lane's original lockfile change chose a value: no entry outside the pinned
crates may take a value that neither side had.

usage: test-lock-delta-only-pin.py NEW_BASE ORIGINAL_TIP REBASED_TIP LOCK PIN_PREFIX
"""
import subprocess
import sys
import tomllib


def lock_entries(rev, path):
    text = subprocess.run(["jj", "--ignore-working-copy", "file", "show", "-r", rev, path],
                          capture_output=True, check=True).stdout.decode()
    return {(p["name"], p["version"], p.get("source", "")): sorted(p.get("dependencies", []))
            for p in tomllib.loads(text)["package"]}


def packages_outside_pin_delta(entries, pin_prefix):
    return {k for k in entries if not k[0].startswith(pin_prefix)}


def main():
    new_base, original_tip, rebased_tip, path, pin_prefix = sys.argv[1:6]
    base, orig, rebased = (lock_entries(r, path) for r in (new_base, original_tip, rebased_tip))
    bad = []
    for key in sorted(packages_outside_pin_delta(rebased, pin_prefix) & set(base)):
        deps = [d for d in rebased[key] if not d.startswith(pin_prefix)]
        allowed = [[d for d in side[key] if not d.startswith(pin_prefix)] for side in (base, orig) if key in side]
        if deps not in allowed:
            bad.append(f"{key[0]} {key[1]}: rebased {deps} not in base/original {allowed}")
    for line in bad:
        print("FAIL:", line)
    print(f"{path}: {len(bad)} entr(y/ies) outside the pin with a value neither side had")
    sys.exit(1 if bad else 0)


main()
