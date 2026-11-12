#!/usr/bin/env python3
"""Fail when a lock file compiles a hickory DNSSEC feature in.

deny.toml ignores RUSTSEC-2026-0514 because no hickory dnssec feature is built
into Holon. Every hickory dnssec feature (`dnssec-ring`, `dnssec-aws-lc-rs` of
hickory-resolver and hickory-proto) turns on hickory-proto's `__dnssec`, and only
`__dnssec` pulls the optional dependencies in MARKERS. A lock file is resolved
with every feature of its workspace members on, so a marker in hickory-proto's
locked dependencies means some build of that workspace can turn DNSSEC on.

  check-hickory-dnssec-off.py DIR...    check DIR/Cargo.lock of each workspace
  check-hickory-dnssec-off.py --self-check   property over generated lock files (CASES=<n>, default 300)
"""

import os
import random
import re
import sys
from pathlib import Path

CRATE = "hickory-proto"
FEATURE = "__dnssec"
# hickory-proto 0.25: `__dnssec = ["dep:bitflags", "dep:rustls-pki-types", "dep:time", "std"]`.
MARKERS = {"bitflags", "rustls-pki-types", "time"}

NAME = re.compile(r'^name = "([^"]+)"$', re.M)
VERSION = re.compile(r'^version = "([^"]+)"$', re.M)
DEPS = re.compile(r"^dependencies = \[\n(.*?)^\]$", re.M | re.S)
DEP = re.compile(r'^\s*"([^" ]+)[^"]*",?$', re.M)


def packages(lock_text):
    """(name, version, dependency names) of every [[package]] in a Cargo.lock text."""
    for block in lock_text.split("[[package]]\n")[1:]:
        deps = DEPS.search(block)
        yield NAME.search(block)[1], VERSION.search(block)[1], DEP.findall(deps[1]) if deps else []


def findings(lock_text):
    return [
        f"{CRATE} {version} depends on {dep}, which only its `{FEATURE}` feature (every hickory dnssec-* feature) enables"
        for name, version, deps in packages(lock_text)
        if name == CRATE
        for dep in deps
        if dep in MARKERS
    ]


def render(pkgs):
    out = ["version = 4\n"]
    for name, version, deps in pkgs:
        out.append(f'\n[[package]]\nname = "{name}"\nversion = "{version}"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n')
        if deps:
            out.append("dependencies = [\n" + "".join(f' "{d}",\n' for d in deps) + "]\n")
    return "".join(out)


NAMES = [CRATE, "hickory-resolver", "hickory-dnssec-tools", "dnssec-prover", "time", "bitflags", "ring", "iroh-relay"]
DEP_POOL = sorted(MARKERS) + ["time-macros", "bitflags-serde", "rustls", "ring", "tokio", "dnssec", "hickory-proto", "pki-types"]
SUFFIXES = ["", " 0.3.41", " 2.9.1 (registry+https://github.com/rust-lang/crates.io-index)"]
# Fixed cases run first: the hickory-proto entry of the tree today (green) and that entry with `time` (red).
TODAY = ["async-trait", "bytes", "cfg-if", "data-encoding", "enum-as-inner 0.6.1", "futures-channel", "futures-io", "futures-util", "h2", "http", "idna", "ipnet", "once_cell", "rand 0.9.5", "ring", "rustls", "thiserror 2.0.21", "tinyvec", "tokio", "tokio-rustls", "tracing", "url"]
EXAMPLES = [
    [(CRATE, "0.25.2", TODAY), ("hickory-resolver", "0.25.2", ["hickory-proto", "time"])],
    [(CRATE, "0.25.2", TODAY + ["time 0.3.41"])],
]


def draw(rng):
    return [
        (rng.choice(NAMES), f"0.{rng.randrange(30)}.{rng.randrange(5)}", [rng.choice(DEP_POOL) + rng.choice(SUFFIXES) for _ in range(rng.randrange(6))])
        for _ in range(rng.randrange(6))
    ]


def self_check():
    rng = random.Random()
    wrong = 0
    cases = EXAMPLES + [draw(rng) for _ in range(int(os.environ.get("CASES", "300")))]
    for pkgs in cases:
        expected = sorted(d.split(" ")[0] for n, _, deps in pkgs if n == CRATE for d in deps if d.split(" ")[0] in MARKERS)
        got = sorted(re.search(r"depends on (\S+),", f)[1] for f in findings(render(pkgs)))
        if got != expected:
            wrong += 1
            print(f"[hickory-dnssec] self-check FAILED: expected {expected}, got {got} for EXAMPLES entry {pkgs!r}", file=sys.stderr)
    print(f"[hickory-dnssec] self-check: {len(cases)} lock files, {wrong} wrong")
    return wrong


def main(argv):
    if argv == ["--self-check"]:
        return 1 if self_check() else 0
    assert argv, "usage: check-hickory-dnssec-off.py DIR... | --self-check"
    failed = False
    for d in argv:
        lock = Path(d) / "Cargo.lock"
        for f in findings(lock.read_text()):
            failed = True
            print(f"[hickory-dnssec] {lock}: {f}", file=sys.stderr)
    if failed:
        print("[hickory-dnssec] FAIL: the deny.toml ignore of RUSTSEC-2026-0514 is void while a hickory dnssec feature is on; turn the feature off or drop the ignore", file=sys.stderr)
        return 1
    print(f"[hickory-dnssec] PASS: no hickory dnssec feature in {len(argv)} lock file(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
