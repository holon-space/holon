---
id: 2026-07-10-creation-slot-after-commit-stale-text
date: 2026-07-10
gap: PERCEPTION
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Creation slot after commit: stale text stays painted OVER the "Type here to
  add a new block" placeholder (persists across Escape/refocus; describe_ui
  says `editable_text ""` while pixels show text — shadow-vs-pixels
  divergence); Enter in the slot also dispatches `split_block` on the
  nonexistent `__virtual:` id ("Block not found")
source_line: 882
---

## Bug

Creation slot after commit: stale text stays painted OVER the "Type here to
add a new block" placeholder (persists across Escape/refocus; describe_ui
says `editable_text ""` while pixels show text — shadow-vs-pixels
divergence); Enter in the slot also dispatches `split_block` on the
nonexistent `__virtual:` id ("Block not found")

## Missing piece

no pixel-level assertion; slot-commit + immediate-Enter sequence not
generatable headless

## Remedy

FIXED (overnight 2026-07-11): TWO defects — (a) Enter capture
unconditionally chained `structural_block_action` → split_block on the
virtual id after the create; now gated on
`RowOrigin::is_creation_placeholder` and dispatches exactly one create via
new `commit_creation_slot` (structural op on a virtual id = loud assert);
(b) nothing cleared the slot's InputState after commit (focus stays on slot,
so the focus-gain convergence never fires) + `pending_commit_intent`
re-baselined to the committed text — now
`converge_input("post_commit_clear")` resets to placeholder and re-baseline
is asserted-empty. 4 new tests incl. exactly-one-create and
retype-identical-creates-again. Follow-up wart noted: slot's per-keystroke
Change handler still dispatches set_field against the virtual id
(pre-existing)
