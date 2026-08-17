---
id: 2026-07-10-org-writeback-writes-siblings-non-sort
date: 2026-07-10
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Org writeback writes siblings in non-sort_key order: disk `Journals.org` has
  c2 before c1 while SQL (`A0` < `A180`) and describe_ui both order c1 first;
  file contains the latest mutation so it is not staleness
source_line: 884
---

## Bug

Org writeback writes siblings in non-sort_key order: disk `Journals.org` has
c2 before c1 while SQL (`A0` < `A180`) and describe_ui both order c1 first;
file contains the latest mutation so it is not staleness

## Missing piece

writeback ordering vs sort_key not asserted at prod wiring (render path
orders differently than the block view)

## Remedy

FIXED (stream 2026-07-10): the incremental writeback cache's cheap
content-only-edit path compared only parent_id/tags — a same-parent reorder
was invisible (domain `Block` doesn't carry sort_key, ADR 0005), so the file
kept stale pre-reorder order forever. Fix: compare cached sibling order
against live `BlockOrdering::children` before trusting the cheap path. Test
`reorder_within_parent_takes_full_reseed`
