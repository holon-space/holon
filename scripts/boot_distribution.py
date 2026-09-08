#!/usr/bin/env python3
"""Boot-time distribution across every vault-scale run in lane-logs/.

Splits the runs by REGIME rather than by filename: a run whose projection
passes are one-full-walk is the fixed code; a run with a full walk per file is
the defect (baseline, or the reverted-DI teeth probe). Runs that predate the
`projection passes` print line are classified from the mode= field of the
holon_latency projection events.
"""

import glob
import os
import re
import statistics
import sys

ANSI = re.compile(r"\x1b\[[0-9;]*m")
BOOT = re.compile(r"\[vault-scale\] boot: (\d+) files / (\d+) blocks in ([0-9.]+)s")
PASSES = re.compile(r"\[vault-scale\] projection passes: (\d+) total, (\d+) full")
MODE = re.compile(r'mode=(?:")?(full|incremental)(?:")?')

rows = []
for path in sorted(glob.glob(os.path.join(sys.argv[1], "*.log"))):
    text = ANSI.sub("", open(path, errors="replace").read())
    boot = BOOT.search(text)
    if not boot:
        continue
    files, blocks, secs = int(boot[1]), int(boot[2]), float(boot[3])
    passes = PASSES.search(text)
    if passes:
        full = int(passes[2])
    else:
        full = sum(1 for m in MODE.finditer(text) if m[1] == "full")
    rows.append((os.path.basename(path), files, blocks, secs, full))

for corpus in sorted({(f, b) for _, f, b, _, _ in rows}):
    files, blocks = corpus
    group = [r for r in rows if (r[1], r[2]) == corpus]
    fixed = sorted(r[3] for r in group if r[4] <= 2)
    defect = sorted(r[3] for r in group if r[4] > 2)
    print(f"\n== {files} files / {blocks} blocks ==")
    for label, xs in (("one-full-walk (fixed)", fixed), ("full-walk-per-file (defect)", defect)):
        if not xs:
            continue
        print(
            f"  {label}: n={len(xs)} min={xs[0]:.1f}s "
            f"median={statistics.median(xs):.1f}s max={xs[-1]:.1f}s"
        )
        print("    samples: " + ", ".join(f"{x:.1f}" for x in xs))
    for name, _, _, secs, full in sorted(group, key=lambda r: r[3]):
        print(f"    {secs:8.1f}s  full={full:<4} {name}")
