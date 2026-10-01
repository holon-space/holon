---
id: 2026-10-01-sqlonly-creation-slot-refusal-under-a-read-only-page-never-reaches-the-caller
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  On the no-cell (SqlOnly) leg, a creation-slot birth under the read-only recipe
  page minted an id, seated the caret in it and dispatched a detached create;
  the dispatcher's refusal went only to an ERROR log, so the headless driver
  waited 3 s for a row that could never land and the reference model predicted
  that the block would land.
---

## Bug
Keystone red registered as `creation-affordance-read-only-focus-root`: 31
timeout births in the source logs, every one under the recipe page
`block:5f54ed1a-…` and on a CRDT-off draw, each one preceded by
`ERROR … Operation block.create failed: … cooklang is a read-only format`.
Root-caused in the red-RCA lane (probe `r1-sql`, 3/3 red in 2 steps).

## Root cause
- `birth_creation_affordance` (`crates/holon-frontend/src/reactive.rs`) minted
  the id, recorded the newborn, seated the caret and returned `Ok(id)` before
  the spawned `block.create` ran. The dispatcher refused it correctly, but the
  refusal went to `surface_op_failure` (ERROR log + op-failure sink), not to
  the caller.
- `ReactiveEngineDriver::commit_creation_slot`
  (`crates/holon-frontend/src/user_driver.rs`) then polled 3 s for the newborn
  and panicked with "did not land within 3s".
- The reference model (`birth_block_via_creation_slot`) did not know that a
  create under a read-only-homed block is refused, so it predicted a landed
  block.

## Missing piece
A tier decision that the birth asks synchronously, before it mints anything,
with a typed outcome the caller can see; and a reference model that knows
which blocks the read-only document owns.

## Remedy
- `WriteTierAuthority::refusal_for` is synchronous. The birth asks it for the
  parent before minting, seating the caret or recording a newborn, on both
  legs, and returns `BirthOutcome::Refused` (disclosed on the condition bus,
  not ERROR-logged). `caret_block_for_edit` returns the typed `EditTarget`;
  `commit_creation_slot` returns at once on a refusal.
- `create_entity_sync` refuses a read-only parent with `Err(EditRefused)`.
- Reference model: `ReadOnlyRefState` maps the page and the steps to their
  file; a birth under one predicts no block, the caret on the affordance, and
  the `edit_refused_read_only_format` condition.
- `inv-read-only-home-refuses-writes` also fails on a `block_raw` row created
  under a refused parent.
- Pinned by the hand-authored rows
  `creation-slot-birth-under-a-read-only-page-is-refused-sql-leg` and
  `…-loro-leg` (red → green, sabotage teeth in the I1 lane logs).
