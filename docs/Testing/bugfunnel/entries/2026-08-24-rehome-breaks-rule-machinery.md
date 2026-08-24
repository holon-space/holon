---
id: 2026-08-24-rehome-breaks-rule-machinery
date: 2026-08-24
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Re-homing a rule's own source block out of the page whose file carries the
  rule succeeds, and the rule stops working; no gate judged where the block
  was allowed to land.
---

## Bug

`block.rehome_entity` (and the `block.move_block` it dispatches) accepted a
rule's `holon_rule` action block as a target and re-parented it to the
no-parent root. A rule is discovered by structure — the `Journal Auto-Create`
heading owning a rule head and its trigger sibling, which is what
`block_profile.yaml`'s `is_program` reads — so moving the action out of that
heading leaves a rule nothing evaluates. The operation reported success.

The op already refused a non-leaf, a block no document holds, and a home that
cannot receive an entity. It refused nothing about *where the block was going*:
no seam in the dispatch path knew what the resulting placement would be.

Found by the keystone during 2b.4 I2b (log
`inc2b i2b-FINAL-81286-20260824-050028.log`).

## Root cause

The dispatcher had two pre-provider gates — `BoundaryEnforcer` (authorization)
and `GuardWorld` (declared `#[require]` predicates) — and neither ranges over
the delta an operation writes. Authorization asks who may act on the subject;
a declared guard asks whether a subject-bound predicate holds now. Placement
legality is a property of the *resulting* marking, so it belonged to neither,
and individual providers cannot own it either: `rehome_entity` performs its
move by dispatching `move_block`, so a check inside one op is not a check on
the other.

## Missing piece

COVERAGE, in the generator. The keystone's `RehomeEntity` transition
deliberately excluded every Source-typed block from its candidate set, with the
stated reason that "a rule's action block moved out of its page stops parsing,
which is a fact about rules, not about re-homing". That reasoning is what kept
the draw away from the incident: the scoping treated the breakage as
out-of-scope instead of as the refusal the system owed.

## Remedy

Closed by the ADR 0032 §3 net gate and its first policy.

- The scoping is relaxed: `candidates` and `preconditions` in
  `crates/holon-integration-tests/src/pbt/transitions/rehome_entity.rs` now
  admit rule machinery, and `apply_to_ref` models the refusal (nothing moves).
  `SutRehomeEntity::rehome_entity` returns a `RehomeOutcome` so a refusal is a
  verdict the model predicts rather than a harness panic.
- `crates/holon/src/api/net_guard.rs` adds the third gate; the policy is
  `crates/holon-app/src/move_guard.rs`.
- Pinned deterministically by the hand-authored case
  `rehome-rule-action-is-refused`.

Red with the gate's wiring removed, at
`crates/holon-integration-tests/src/pbt/transitions/rehome_entity.rs:161`:

```
re-homing block:journals::action::0 : expected a refusal, because the block is
rule machinery, got Moved
```
