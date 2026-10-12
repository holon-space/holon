---
id: 2026-10-12-cyclic-place-dispatches-unresolved-oracle-ids
date: 2026-10-12
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The keystone's cyclic-place dispatch sent the oracle's synthetic
  `block::create-N` ids to the production `move_block`, so the net guard
  correctly refused a destination no store holds and the transition's
  wrong-reason assertion fired — a harness id-translation miss, not a
  product defect.
---

## Bug

The composed keystone (`general_e2e_composed_pbt`) died on the first draw on
main `53c5b430` (holon-work#182, part of #163):

```
[cyclic place] move_block(block:c1 under its descendant block::create-0) was
refused for the wrong reason.
 expected: Cannot move block:c1 under block::create-0 …
 got: net guard: destination `block::create-0` is not a block the store holds
```

Filed as a prod-bug candidate of the
`2026-09-23-net-guard-and-task-keyword-read-the-lagging-projection` family (the
guard reading the lagging projection). It is not one.

## Root cause

`block::create-N` is an ORACLE-ONLY id. `CreateBlockUnderFocus`'s `id: None`
arm mints it in the reference state
(`crates/holon-integration-tests/src/pbt/reference_state.rs:2493`) while the
SUT's `block.create` mints a real uuid; the harness pairs the two in the run's
`IdResolver` (`crates/holon-integration-tests/src/pbt/composed/harness.rs:125`),
and every id-taking SUT cap translates through
`HeadlessFrontendComponent::resolve_id`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:1692`).

`SutCyclicPlaceAttempt::attempt_place_under_own_descendant`
(same file, line 8809) was the one id-taking cap that skipped that
translation: it put `id` and `descendant` into the `move_block` params
verbatim. So the destination parameter was an id no store ever held, and
`MoveGuard::destination_format`
(`crates/holon-app/src/move_guard.rs:181`) refused it — correctly, after
`await_projection_of` had confirmed the write authority does not hold it
either. The guard, the projection and the cycle check are all sound; the
dispatch was.

The product's cyclic refusal is unaffected: the two hand-authored pins of
`0626a77b33` (`place-under-own-descendant-refused-on-the-sql-authority`,
`…-on-loro`) create their blocks with explicit born-equal ids
(`block:mp0`/`block:mp1`), where `resolve_id` is the identity — which is
exactly why they stayed green while the drawn alphabet went red.

## Missing piece

Nothing in the product. In the harness: no pin drove
`PlaceUnderOwnDescendant` over slot-minted (`id: None`) blocks, the only
shape in which the missing translation is observable.

## Remedy

`attempt_place_under_own_descendant` resolves both ids through `resolve_id`
before dispatching (and the expected-refusal text is built from the resolved
ids, so the assertion quotes the ids production saw).

Pinned deterministically by the hand-authored case
`place-under-own-descendant-with-slot-minted-ids`
(`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`):
two slot births, an indent, then the cyclic place — red with the write-authority
refusal before the fix, green after.
