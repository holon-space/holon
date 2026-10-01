---
id: 2026-10-01-model-seats-the-caret-on-a-read-only-pages-slot
date: 2026-10-01
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  After navigation to a read-only page the reference model seats the caret on
  the page's creation slot, but the app seats it on the first recipe step:
  the model holds no read-only children, and no slot is offered there.
---

## Bug
Found by the lane I1 agents (round 2b/2c, while writing the read-only-page
keystone rows). Not seen by a user. The app is correct; the model is wrong.

## Root cause
`ReferenceState::caret_seat_for_navigation`
(`crates/holon-integration-tests/src/pbt/reference_state.rs`) picks the
destination's first child from the model's block tree, else its creation
affordance. The model does not hold the blocks a read-only document (the
keystone `.cook` recipe) brings in, so for the recipe page it finds no child
and predicts the slot. The engine (`navigation_caret_target` in
`crates/holon-frontend/src/reactive.rs`) asks the query backend and seats the
first recipe step. With the no-slot design the model's prediction names a slot
that is never drawn.

## Missing piece
No invariant compares the caret after a navigation to a read-only page before
the next focus move; the keystone rows refocus `block:keep` right after the
navigation, and a `TypeChars` there lands in the known red
`cooklang-read-only-write-refusal-any-op`.

## Remedy
OPEN. Not a one-line change: the model must know the read-only page's first
child (seed the recipe's blocks into the model, or have the read-only-homes
capability answer the page's first block). Then pin with a keystone row that
navigates to the recipe page and checks the caret, red before the fix.
