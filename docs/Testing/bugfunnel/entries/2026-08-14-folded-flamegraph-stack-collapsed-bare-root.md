---
id: 2026-08-14-folded-flamegraph-stack-collapsed-bare-root
date: 2026-08-14
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Every folded flamegraph stack collapsed to a bare root
source_line: 706
---

## Bug

(task-#18 flamegraph lane; found by the OTel research lane reading the
folded output, `lane-logs/research-otel-perf.md` P4) **Every folded
flamegraph stack collapsed to a bare root**: `maybe_write_flamegraph`
filtered the window to the perf-span allowlist and then handed only that
slice to `write_folded_stacks`, whose `parent_span_id` index is built from
the slice it receives — so the walk stopped at the first non-allowlisted
ancestor and `query` lines were written as `query 1389` instead of
`frontend.render;resolve_doc;query 1389`.

## Root cause

task-#18 flamegraph lane, found by the OTel research lane READING the folded
output (`lane-logs/research-otel-perf.md` P4) — no test produced it: **every
folded flamegraph stack collapsed to a bare root, so the profiles attributed
nothing.** `maybe_write_flamegraph` filtered the window down to the
perf-span allowlist and only THEN handed that filtered slice to
`write_folded_stacks`, which builds its `parent_span_id` index from the
slice it is given — so any ancestor outside the allowlist (`resolve_doc`,
transition spans, CDC plumbing) was missing from the index and the walk
stopped there. `query` lines came out as `query 1389` instead of
`frontend.render;resolve_doc;query 1389`.
`queries_by_origin`/`find_duplicate_sql` in the same file always walked the
FULL window and filtered only at emission, so the origin tables were right
while the flamegraphs silently were not. The escape is an oracle gap:
`repeated_writes_for_one_key_keep_both_files` already drove the writer, but
it asserted only the file COUNT — nothing asserted what a folded line
contains. FIXED: `write_folded_stacks(all_spans, names, path)` now mirrors
`find_duplicate_sql`'s shape, walking ancestry over the whole window and
emitting only `PERF_SPAN_NAMES`; pinned by
`folded_stacks_keep_ancestors_that_are_not_perf_spans`, which feeds a
non-allowlisted intermediate and demands the full chain.)

## Missing piece

`repeated_writes_for_one_key_keep_both_files` drove the writer but asserted
only the file count; nothing asserted what a folded line contains, while
`queries_by_origin` and `find_duplicate_sql` in the same file walked the
full window correctly.

## Remedy

FIXED — `write_folded_stacks(all_spans, names, path)` mirrors
`find_duplicate_sql`: ancestry over the whole window, emission restricted to
`PERF_SPAN_NAMES`; pinned by
`folded_stacks_keep_ancestors_that_are_not_perf_spans` (red before the fix:
the folded file held only `query 1389` / `frontend.render 2505`).
