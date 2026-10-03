---
id: 2026-10-03-undo-of-a-content-edit-on-a-source-block-is-refused
date: 2026-10-03
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A plain content edit of a source-typed block is accepted, but its recorded inverse always
  goes through the marked-content write, which refuses source blocks, so the undo fails.
---

## Bug

Found by a verifier's full-core run (`verify-reds/full-core.log`, test
`undo_prod_session_wiring::prod_session_wires_undo_and_mcp_shares_the_same_stack`), not by a
person. The test edited a block, then called `session.undo()`. The undo returned an error:

`undo: composite inverse op 0 of 1 ('set_field' on 'block') failed — stopping (partial undo,
earlier inverses already applied): undo/redo replay of 'set_field' failed: Failed to update
marked content: Internal error: Failed to update block marked: update_block_marked: source
blocks cannot carry inline marks`

This is a second failure shape of the same test as the registered
`undo-prod-session-stale-dropped` row (`KeystoneKnownReds.md`).

## Root cause

Code reading plus the log line. Not reproduced on demand.

- Forward write: `set_field("content", Value::String)` calls `update_block_text`, which accepts a
  source block (`loro_block_operations.rs`, the `Value::String` arm of the `content` match).
- Recorded inverse: the same `set_field` builds its inverse with `rich_content_restore_value`,
  which is a `Value::Object { text, marks }` (`loro_block_operations.rs`, `"content" if
  matches!(value, Value::String(_))`). Replay takes the `Value::Object` arm, which calls
  `update_block_marked`.
- `update_block_marked` refuses a source block (`loro_backend.rs:3476`).

So every undo of a plain content edit on a source-typed block fails, and a composite undo stops
half-applied ("partial undo" in the message).

The test is intermittent because `pick_target` takes the first match from an unordered
`HashMap` iteration and does not exclude source blocks (it only excludes ids that contain
`::src::` / `::render::`). Whether a source block is drawn varies per process.

## Missing piece

No generated or pinned case undoes a content edit on a source-typed block. The keystone catalog
was not checked for such a transition; this entry does not claim that none exists. The test's
subject selection is also non-deterministic, so the defect shows as a flake.

## Remedy

OPEN. Options, not decided: the inverse for a source block restores with a text-only
`set_field`; or `update_block_marked` accepts empty `marks` on a source block. Either is a
behaviour change and needs the `holon-feature` red-first test. Until then the registry row
`undo-prod-session-source-block-marks` classifies the shape.
