---
id: 2026-09-09-dioxus-web-editor-never-resolves-a-slot-caret
date: 2026-09-09
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The dioxus-web editor builds its structural intents from the raw row id, so a
  caret seated on a creation affordance (D97.a) trips the fail-loud
  placeholder assertion instead of birthing a block.
---

## Bug

Found by reading, not by running: the `quick-open-caret` lane routed every
GPUI and headless commit funnel through `ViewEventHandler::edit_target_id`
(D97.a) and left `frontends/dioxus-web/src/editor.rs:347` naming
`entity_id` — the row id as rendered — directly.

Under D97.a a `navigation.focus` into main seats the caret on the
destination's `block:__virtual:<id>` creation affordance whenever the
destination has no children. On that row, Enter / Backspace / Tab in the
dioxus-web editor reach `structural_block_action` with an affordance id.

It **fails loud** rather than corrupting anything: the placeholder assertion at
`crates/holon-frontend/src/editor_view_model.rs:1307` panics. So the symptom is
a crashed web editor, not a silent write against an id the backend has no block
for.

## Root cause

`edit_target_id` resolves an affordance caret through
`BuilderServices::caret_block_for_edit`, the idempotent birth chokepoint. The
dioxus-web editor holds no `BuilderServices`: it dispatches over the wire
(`dispatch_chain` / `intent_to_wire`), so the same resolution needs a wire op
rather than the one-line call GPUI's `structural_target` makes.

## Missing piece

No PBT drives the dioxus-web editor at all — there is no rung for it in the
keystone's driver ladder, so no generated sequence can reach a slot caret
there. COVERAGE, not ORACLE: the assertion that would catch it exists and is
loud; nothing can generate the interaction.

## Remedy

OPEN. Two halves, in order:

1. Give the wire dispatch a "resolve the caret's edit target" op so the
   dioxus-web editor can reach the same chokepoint.
2. Add the dioxus-web rung to the driver ladder, so a slot caret is reachable
   there and this stops being unreachable-by-construction.

Until then the fail-loud assertion IS the record: a slot caret in that editor
panics visibly instead of writing against an affordance id.
