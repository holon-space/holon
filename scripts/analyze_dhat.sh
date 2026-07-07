#!/usr/bin/env bash
# Summarize a dhat-heap.json WITHOUT the web viewer (offline-friendly).
# Prints lifetime total bytes/allocations + the top allocation sites by total
# bytes, naming the first *meaningful* stack frame (allocator-internal frames —
# dhat's shim, __rust_alloc, RawVec/Vec growth — are skipped so the caller
# shows). For the full flame view open the file at
# https://nnethercote.github.io/dh_view/dh_view.html
set -euo pipefail
f="${1:-dhat-heap.json}"
n="${2:-15}"
[ -f "$f" ] || { echo "no such file: $f" >&2; exit 1; }
echo "== dhat summary: $f =="
jq -r --argjson n "$n" '
  # frames considered allocator plumbing, not a real allocation site
  def noise: test("dhat-0\\.|__rust_alloc|__rust_realloc|alloc::alloc|raw_vec|RawVec|alloc::vec::Vec|GlobalAlloc|<alloc::|Allocator::|realloc|reserve");
  .ftbl as $ft
  | ([.pps[].tb] | add) as $total
  | ([.pps[].tbk] | add) as $blocks
  | "total bytes (lifetime alloc): \($total)",
    "total allocations (blocks):   \($blocks)",
    "",
    "top \($n) sites by total bytes (first non-plumbing frame):",
    ( .pps | sort_by(.tb) | reverse | .[0:$n][]
      | . as $pp
      | ([ $pp.fs[] | $ft[.] | select(noise | not) ][0] // ($ft[$pp.fs[0]] // "<root>"))
      | . as $frame
      | "  \($pp.tb) B  \($pp.tbk) blk  \($frame | sub("^0x[0-9a-f]+: ";""))" )
' "$f"