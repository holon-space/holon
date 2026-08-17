---
id: 2026-07-11-click-caret-end-contract-breaks-after
date: 2026-07-11
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Click-to-caret-at-end contract breaks after split+join (and after undo/redo
  + a failed click elsewhere): caret resets to position 0, subsequent typing
  PREPENDS into the block instead of appending (dogfood #3, control click on
  untouched block behaves)
source_line: 894
---

## Bug

Click-to-caret-at-end contract breaks after split+join (and after undo/redo
+ a failed click elsewhere): caret resets to position 0, subsequent typing
PREPENDS into the block instead of appending (dogfood #3, control click on
untouched block behaves)

## Missing piece

keystone never chains split→join→click→type; no caret-position invariant
after join/undo cycles

## Remedy

FIXED (code). Root cause: the op-follow-up caret seed
(`UiState::pending_caret_seed`, armed by split→0 / join→boundary /
nav→offset) was applied by the mounting GPUI editor via `peek_caret_seed`
(non-destructive) but NEVER consumed. It lingered, aged ONLY by a focus MOVE
to a *different* block (`set_focus`), so after a "failed click elsewhere"
(window blur without a focused-block change) it stayed armed for the same
block; the next click on it re-applied the stale offset (0 after an
undone/redone split), yanking the caret to 0 → typing PREPENDED. Remedy:
made the seed strictly single-use — new `UiState::consume_caret_seed(block)`
(clears the seed iff it targets `block`, even while focus stays put)
surfaced through `BuilderServices`; `grab_focus_and_seed_caret` in
`frontends/gpui/src/views/editor_view.rs` calls it right after applying the
seed, so whichever of the sync first-mount grab / async focus-subscription
runs first applies AND clears, the other sees nothing, and a later click
always derives the caret from the click/buffer, never the stale offset.
Pinned by
`reactive::tests::caret_seed_is_single_use_so_a_later_click_is_not_yanked`
(+ `consume_caret_seed_is_scoped_to_its_block`) at the seed-lifecycle seam.
ENVIRONMENT/COVERAGE residue OPEN: the caret APPLY needs a live GPUI window,
so the headless keystone still cannot reproduce end-to-end (`click_entity`
falls back to `set_focus` + the headless mirror models caret-to-end,
ignoring a stale seed — same environment gap as row 65); the catching rung
is the live-MCP GPUI twin, and the split→join→click→type chain plus a "caret
position matches ref after type" oracle must be added there, not headlessly
(a headless case would be green either way).
