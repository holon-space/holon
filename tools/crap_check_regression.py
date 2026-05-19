#!/usr/bin/env python3
"""Fail only on real CRAP regressions, comparing two `cargo crap --format json` runs.

Why this exists instead of `cargo crap --fail-regression`:
  cargo-crap's built-in baseline matcher pairs functions by (file, function)
  name and ignores the line number. This codebase has many duplicate-named
  functions across impl blocks and trait impls (two `watch_editor_cursor`,
  three `create_task`, dozens of `new`/`default`/`from`/`fmt`). The built-in
  matcher pairs a complex function against a trivial namesake and reports tens
  of spurious regressions even when the input is byte-for-byte identical.

How this checker matches:
  We can't key on line numbers either — functions legitimately shift lines when
  code above them changes, which would make every edit look like churn. Instead,
  for each (file, function) group we compare the *sorted multiset* of CRAP
  scores: baseline [2, 156] vs current [2, 156] is unchanged; a real complexity
  increase shows up as some current score exceeding its positional baseline
  counterpart by more than --epsilon. Extra current entries (new overloads) are
  reported as "new", never as regressions.

Exit status: 0 = no regressions, 1 = at least one regression, 2 = usage error.
"""

import argparse
import json
import sys
from collections import defaultdict


def load_groups(path):
    """Map (file, function) -> sorted list of CRAP scores."""
    with open(path) as f:
        entries = json.load(f)["entries"]
    groups = defaultdict(list)
    for e in entries:
        groups[(e["file"], e["function"])].append(float(e["crap"]))
    for key in groups:
        groups[key].sort()
    return groups


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--baseline", required=True, help="committed baseline JSON")
    ap.add_argument("--current", required=True, help="freshly generated JSON")
    ap.add_argument(
        "--epsilon",
        type=float,
        default=0.5,
        help="ignore CRAP increases at or below this (absorbs coverage float jitter)",
    )
    args = ap.parse_args()

    baseline = load_groups(args.baseline)
    current = load_groups(args.current)

    regressions = []
    new_funcs = []
    for key, cur_scores in current.items():
        base_scores = baseline.get(key, [])
        # Pair sorted scores positionally; surplus current entries are "new".
        for i, cur in enumerate(cur_scores):
            if i < len(base_scores):
                if cur - base_scores[i] > args.epsilon:
                    regressions.append((key, base_scores[i], cur))
            elif cur > args.epsilon:
                new_funcs.append((key, cur))

    if new_funcs:
        print(f"★ {len(new_funcs)} new function(s) over baseline coverage:")
        for (file, func), cur in sorted(new_funcs, key=lambda x: -x[1])[:15]:
            print(f"    CRAP {cur:8.1f}  {func}  ({file})")

    if regressions:
        print(f"\n↑ {len(regressions)} CRAP regression(s) vs baseline:")
        for (file, func), base, cur in sorted(regressions, key=lambda x: x[1] - x[2]):
            print(f"    {base:8.1f} → {cur:8.1f}  (Δ{cur - base:+.1f})  {func}  ({file})")
        print("\nA function got more complex relative to its test coverage.")
        print("Add tests or simplify it, or run `just crap-baseline` to accept.")
        return 1

    print("✓ No CRAP regressions vs baseline.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
