#!/usr/bin/env python3
"""Aggregate the identity-transfer corpus TSV into per-shape, per-strategy rates.

Usage: identity-transfer-rates.py <corpus.tsv> <out.md> [strategy,strategy,...]
"""
import csv
import sys
from collections import defaultdict

MISBIND = ("misbind_equal", "misbind_move", "misbind_hunk", "misbind_document", "misbind_unique")
src, dst = sys.argv[1], sys.argv[2]
groups = defaultdict(list)
with open(src) as f:
    for r in csv.DictReader(f, delimiter="\t"):
        for k in ("ambiguous", "expected", "correct", "nonpair", "lost_untouched",
                  "misbind_equal", "misbind_move", "misbind_hunk", "misbind_document", "misbind_unique"):
            r[k] = int(r[k])
        r["misbind"] = sum(r[k] for k in MISBIND)
        groups[(r["shape"], r["strategy"], r["ambiguous"])].append(r)
        groups[("ALL", r["strategy"], r["ambiguous"])].append(r)

shapes = ["InPlaceEdit", "DeleteAdjacentEdit", "TwoSimilar", "EditReorder", "RandomMix", "ALL"]
strategies = sys.argv[3].split(",") if len(sys.argv) > 3 else list(dict.fromkeys(k[1] for k in groups))


def pct(n, d):
    return f"{100.0 * n / d:.2f} %" if d else "n/a"


lines = []
for amb, title in ((0, "Unambiguous cases (kill criterion applies)"), (1, "Ambiguous cases (information only)")):
    lines.append(f"### {title}\n")
    lines.append("| shape | strategy | cases | cases with a mis-bind | mis-bound lines | "
                 "mis-binds by source equal/move/hunk/doc/downstream | cases with a non-pair | non-paired lines | "
                 "untouched lines that lost their id |")
    lines.append("|---|---|---|---|---|---|---|---|---|")
    for shape in shapes:
        for strat in strategies:
            rows = groups.get((shape, strat, amb), [])
            n = len(rows)
            exp = sum(r["expected"] for r in rows)
            mb_cases = sum(1 for r in rows if r["misbind"])
            mb_lines = sum(r["misbind"] for r in rows)
            np_cases = sum(1 for r in rows if r["nonpair"])
            np_lines = sum(r["nonpair"] for r in rows)
            src_split = "/".join(str(sum(r[k] for r in rows))
                                 for k in MISBIND)
            lost = sum(r["lost_untouched"] for r in rows)
            lines.append(f"| {shape} | {strat} | {n} | {mb_cases} ({pct(mb_cases, n)}) | "
                         f"{mb_lines} ({pct(mb_lines, exp)}) | {src_split} | "
                         f"{np_cases} ({pct(np_cases, n)}) | {np_lines} ({pct(np_lines, exp)}) | {lost} |")
    lines.append("")

with open(dst, "w") as f:
    f.write("\n".join(lines))
