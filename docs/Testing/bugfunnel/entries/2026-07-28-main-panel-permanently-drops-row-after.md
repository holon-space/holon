---
id: 2026-07-28-main-panel-permanently-drops-row-after
date: 2026-07-28
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Main panel PERMANENTLY DROPS one row after a NavigateFocus-away-and-back
  around a SplitBlock/BlockToPage sequence: the block exists in `block_raw`,
  the `block` matview, Loro and the org file (`inv-blocks-match-ref` 15/15
  green), its siblings render, the panel is otherwise fully materialised — and
  it renders NO node at all, so its `state_toggle` click times out (that is
  the same bug's other face). The oracle could not see it:
  `inv-main-panel-rows-match-focus` asserted only rendered ⊆ allowed, so a
  MISSING row was structurally invisible; it passed 15/15 on every failing
  run. Strengthened to SET EQUALITY — every `main_editable_descendants` row
  must render (rows under an embedded page excluded, that is
  `inv-embedded-page-collapsed-lazy`'s gate) — and it now fires 3/3 on the
  committed hand-authored case and reds `keystone-smoke` on random walks. Root
  cause, traced with a per-CDC-change lifecycle log: the NavigateHome prune
  emits `Deleted` for all 12 rows of the old focus subtree, and the
  NavigateFocus-back batch emits `Created` for 11 of them plus 2 new ones —
  the 12th is never re-asserted, so the row set loses it forever. Frontend
  fully exonerated: generation guard 0 drops, `retain_keys` 0 evictions, and
  provider-rows == driver `row_map` == `MutableTree` == rendered nodes at
  EVERY `VecDiff` boundary (0 divergences);
  `inv-matview-consistent-with-recompute` GREEN in the same run, so the
  matview content is correct and only the delta stream lost the insert. Dual
  of the 2026-07-27 retract-MISS row (retained stale row) — same
  `watch_view_*` recursive matview, same vendored turso IVM.
source_line: 1114
---

## Bug

Main panel PERMANENTLY DROPS one row after a NavigateFocus-away-and-back
around a SplitBlock/BlockToPage sequence: the block exists in `block_raw`,
the `block` matview, Loro and the org file (`inv-blocks-match-ref` 15/15
green), its siblings render, the panel is otherwise fully materialised — and
it renders NO node at all, so its `state_toggle` click times out (that is
the same bug's other face). The oracle could not see it:
`inv-main-panel-rows-match-focus` asserted only rendered ⊆ allowed, so a
MISSING row was structurally invisible; it passed 15/15 on every failing
run. Strengthened to SET EQUALITY — every `main_editable_descendants` row
must render (rows under an embedded page excluded, that is
`inv-embedded-page-collapsed-lazy`'s gate) — and it now fires 3/3 on the
committed hand-authored case and reds `keystone-smoke` on random walks. Root
cause, traced with a per-CDC-change lifecycle log: the NavigateHome prune
emits `Deleted` for all 12 rows of the old focus subtree, and the
NavigateFocus-back batch emits `Created` for 11 of them plus 2 new ones —
the 12th is never re-asserted, so the row set loses it forever. Frontend
fully exonerated: generation guard 0 drops, `retain_keys` 0 evictions, and
provider-rows == driver `row_map` == `MutableTree` == rendered nodes at
EVERY `VecDiff` boundary (0 divergences);
`inv-matview-consistent-with-recompute` GREEN in the same run, so the
matview content is correct and only the delta stream lost the insert. Dual
of the 2026-07-27 retract-MISS row (retained stale row) — same
`watch_view_*` recursive matview, same vendored turso IVM.

## Missing piece

An oracle that reads the RESOLVED ViewModel in both directions (now added),
plus a turso-side repro of an UNPAIRED RETRACTION under a focus-root swap —
the existing sequential SQL rungs model retract-misses, not
retract-without-reassert.

## Remedy

OPEN 2026-07-28 — oracle strengthened + three self-diagnosing probes landed
(`reactive::tree_desync`, `reactive::row_lifecycle`,
`HOLON_TRACE_ROW_LIFECYCLE`); prod fix is turso-side (vendored IVM), so the
repro case `main-panel-drops-refocused-split-block` STAYS in the
hand-authored skip list and `keystone-smoke` now reds on this signature.
Layer attribution ESCALATED: the lane brief assumed a frontend lost-update
on panel rebuild; the evidence refutes that. → FIXED 2026-07-28: same turso
fix stack 80ed4a4a covers the insert-miss dual — ~2/4→0/30 red over 30 fresh
processes with the set-equality oracle engaged (15/15 per tick certified);
case UN-QUARANTINED (PR #129), keystone-smoke green on this signature
