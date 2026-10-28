#!/usr/bin/env python3
"""Fail when a long test's nextest timeout is under twice its standalone time.

A test whose standalone time is over half its timeout times out as soon as the
host is loaded, and the TIMEOUT reads as a failure of the code under test. Every
such test is listed in `MARGINS` below with its standalone time, and needs its
own `test(=<name>)` override in `.config/nextest.toml` whose
`period x terminate-after` is at least `MARGIN x` that time.

  check-timeout-margins.py [NEXTEST.toml]   default: .config/nextest.toml
  check-timeout-margins.py --self-check     prove the check fails a short timeout

A test's timeout is the first override with a `slow-timeout` that names it as
`test(=<name>)`, in the order nextest applies them for the load-tolerant
profile the lanes run: that profile's overrides, then the default profile's.
shortcut: an earlier override that matches the test by a broader filter is not
detected, because evaluating filters needs the built test binaries; upgrade to
`cargo nextest list` if a listed test ever gets shadowed.
"""

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
NEXTEST = REPO / ".config/nextest.toml"
MARGIN = 2
PROFILE_ORDER = ("load-tolerant", "default")

# test name -> (standalone seconds, how it was measured). Standalone means
# `--test-threads 1`; the load is the host's 1-min load during the test.
MARGINS = {
    "pbt::composed::wide_e2e::tests::sqlonly_typing_share_meets_its_floor_beside_the_loro_control":
        (585, "584 s at load 20-85 and 497 s at load 51-180, 2026-10-27"),
    "catalog_suite::dogfood_recorded_replay::recorded_features_replay_over_the_composed_sut":
        (500, "485-500 s reported standalone 2026-10-17; 131 s at load 20-27 and 204 s at load 61-205, 2026-10-27"),
    "catalog_suite::logseq_parity_replay::logseq_parity_corpus_replay":
        (319, "319 s reported standalone 2026-10-17; it reds before its last scenario on 2026-10-27"),
    "cook_vault_ingest::the_default_layout_seed_does_not_wait_for_the_recipe_backlog":
        (169, "122 s at load 16-24 and 169 s at load 45-78, 2026-10-27"),
}

HEADER = re.compile(r"^\[\[profile\.([\w-]+)\.overrides\]\]\s*$")
FILTER = re.compile(r'^filter\s*=\s*"(.*)"\s*(#.*)?$')
SLOW = re.compile(
    r'^slow-timeout\s*=\s*\{\s*period\s*=\s*"(\d+)s"\s*,\s*terminate-after\s*=\s*(\d+)\s*\}\s*(#.*)?$'
)


def overrides(text):
    """(profile, filter, timeout_s or None) per override block, in file order."""
    out = []
    current = None
    for line in text.splitlines():
        line = line.strip()
        m = HEADER.match(line)
        if m:
            current = {"profile": m.group(1), "filter": None, "timeout": None}
            out.append(current)
            continue
        if line.startswith("["):
            current = None
            continue
        if current is None:
            continue
        if m := FILTER.match(line):
            current["filter"] = m.group(1)
        elif line.startswith("slow-timeout"):
            m = SLOW.match(line)
            if not m:
                raise SystemExit(f"[timeout-margins] cannot parse: {line}")
            current["timeout"] = int(m.group(1)) * int(m.group(2))
    return out


def timeout_of(test, blocks):
    selector = f"test(={test})"
    for profile in PROFILE_ORDER:
        for b in blocks:
            if b["profile"] == profile and b["timeout"] and selector in (b["filter"] or ""):
                return b["timeout"]
    return None


def check(text, margins):
    blocks = overrides(text)
    failures = 0
    for test, (standalone, measured) in margins.items():
        timeout = timeout_of(test, blocks)
        if timeout is None:
            print(f"FAIL: {test}: no override with a slow-timeout names `test(={test})`")
            failures += 1
        elif timeout < MARGIN * standalone:
            print(f"FAIL: {test}: timeout {timeout}s < {MARGIN} x standalone {standalone}s ({measured})")
            failures += 1
        else:
            print(f"ok: {test}: timeout {timeout}s >= {MARGIN} x standalone {standalone}s")
    return failures


def self_check():
    margins = {"t::long": (300, "fixture")}
    # name -> (override profile or None, terminate-after at period 60s, failures wanted)
    cases = {
        "under": ("default", 9, 1),
        "at": ("default", 10, 0),
        "over": ("load-tolerant", 11, 0),
        "missing": (None, 0, 1),
    }
    wrong = 0
    for name, (profile, terminate_after, want) in cases.items():
        text = ""
        if profile:
            text = (f"[[profile.{profile}.overrides]]\n"
                    f'filter = "binary(x) & test(=t::long)"\n'
                    f'slow-timeout = {{ period = "60s", terminate-after = {terminate_after} }}\n')
        got = check(text, margins)
        ok = (got > 0) == (want > 0)
        print(f"[timeout-margins] self-check {name}: {got} failure(s), expected {want}")
        wrong += not ok
    return wrong


def main(argv):
    if argv == ["--self-check"]:
        wrong = self_check()
        print(f"[timeout-margins] self-check {'FAILED' if wrong else 'passed'}")
        return 1 if wrong else 0
    path = pathlib.Path(argv[0]) if argv else NEXTEST
    failures = check(path.read_text(), MARGINS)
    print(f"[timeout-margins] {len(MARGINS)} listed test(s), {failures} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
