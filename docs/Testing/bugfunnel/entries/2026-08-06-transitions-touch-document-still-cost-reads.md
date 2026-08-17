---
id: 2026-08-06-transitions-touch-document-still-cost-reads
date: 2026-08-06
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Transitions that touch no document still cost reads, and the cost is
  STATE-CONDITIONAL.
source_line: 1160
---

## Bug

(budget-lane armed-gate investigation) **Transitions that touch no document
still cost reads, and the cost is STATE-CONDITIONAL.** `Nothing` — a literal
no-op — measures 0 reads in 47/57 samples (all at d=1) and 3 reads in 10/57
(all at d=3): the floor appears only once the vault holds several documents,
so it is a function of state, not a constant. `WheelScroll` (pure viewport
motion) measures 3 in all 5 retained samples. Both budgets declared 0 reads
/ 0 tolerance. The composition of the 3 reads is NOT attributed — the
per-region `focus_roots` form once assumed is a test-only fixture; the sole
prod focus_roots read is one un-filtered watch
(`crates/holon/src/sync/turso_block_query_source.rs:134`).

## Missing piece

Same unarmed-gate root cause as the row above: a transition that touches no
document still costs SQL, and nothing measured it. Missing piece = the
budget model has no term for the per-transition reactive floor, and — the
more interesting fact — that floor SCALES WITH DOCUMENT COUNT (0 at d=1, 3
at d=3), so it cannot be pinned as a constant either; it needs a
docs-dependent term the model has nowhere to put.

## Remedy

FIXED 2026-08-06 for the one measured case: `WheelScroll` pinned at 3 with
zero tolerance as a MEASURED CEILING (the three reads are not attributed)
(`crates/holon-integration-tests/src/pbt/transitions/wheel_scroll.rs`).
`Nothing` and the general floor stay OPEN → task #15. Related finding fixed
in the same pass: `NavigateFocus` measured flat at 26 reads regardless of
`last_navigate_first_visit()`, whose predicted 6 CREATEs never fire (ddl=0
in 98/98 samples, including all 43 first-visit ones) — the branch and
`FIRST_VISIT_VIEW_{READS,DDL}` deleted as dead fiction.
