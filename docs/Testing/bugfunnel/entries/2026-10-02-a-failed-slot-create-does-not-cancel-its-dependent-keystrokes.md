---
id: 2026-10-02-a-failed-slot-create-does-not-cancel-its-dependent-keystrokes
date: 2026-10-02
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  When the creation slot's `block.create` fails, every keystroke typed into the slot still
  dispatches its own `set_field` against a block that never existed, so the user gets one
  disclosed error per keystroke instead of one error for the failed create.
---

## Bug

Found by the fresh-context verifier of admission Inc 0a (2026-10-02, verdict
`adm0a-verify/verify.md`, finding F6; lane state `lane-logs/adm0a2-state.md` section "ROUND 3e").
The Inc 0a test-only dispatch hold (`dispatch-hold` feature, `crates/holon/src/api/dispatch_hold.rs`)
can make the slot's `block.create` fail (`effect=Fail`). The keystrokes typed afterwards are not
cancelled: each `set_field` runs against the missing block and fails on its own.

This entry covers BOTH known-red rows `failed-slot-create-sqlonly-keystrokes-run-anyway` and
`failed-slot-create-loro-dispatch-keystrokes-run-anyway`
(`docs/Testing/KeystoneKnownReds.md`, rows 115 and 116). They are one defect on two store legs: the
dispatcher admits a dependent write with no knowledge of the create it depends on. Only the error
text differs (SQL `matched no row in block_raw`; Loro `capture prior state: Block not found`).
Owner: admission Inc 2.

## Root cause

Nothing links a slot's first keystroke write to the slot's create. `birth_creation_affordance`
(`crates/holon-frontend/src/reactive.rs`) spawns the create detached, and every later `set_field`
is an independent fire-and-forget dispatch, so a create failure cannot cancel them. The reference
model has no notion of a cancel set either.

## Missing piece

No failure injection into the create existed before the dispatch hold. The Inc 0a hand-authored rows
`a-failed-create-cancels-its-keystrokes-with-one-disclosure` and
`a-failed-create-cancels-its-keystrokes-with-one-disclosure-loro-dispatch` now reproduce it (red
logs are cited in the two registry rows). The composed keystone has no `FailNextDispatch`
transition in its generated alphabet, and its oracle does not predict a cancel set. {Loro + cell}
is not drawn: the cell-leg birth does not go through the dispatcher.

## Remedy

OPEN, admission Inc 2. Rung that closes it: the two hand-authored rows above go green when a
failed create cancels its dependent writes with ONE disclosure, with the reference predicting the
cancel set. Then draw `FailNextDispatch{block, create}` in the keystone so the cancel set is
checked on random sequences. Registry rows are retired in the same change.
