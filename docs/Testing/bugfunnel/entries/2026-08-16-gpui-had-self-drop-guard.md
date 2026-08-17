---
id: 2026-08-16-gpui-had-self-drop-guard
date: 2026-08-16
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  GPUI's `drop_zone` had no self-drop guard
source_line: 694
---

## Bug

(D20 shared-moves lane; same CODE AUDIT, finding 5 — DESKTOP side) **GPUI's
`drop_zone` had no self-drop guard**, so releasing a drag on a block's own
drop zone dispatched `move_block{id: S, parent_id: S}` — asking the engine
to make a block its own parent. The dioxus-web copy had the guard; GPUI
never grew one.

## Root cause

D20 shared-moves lane, same CODE AUDIT, finding 5 — this one on the DESKTOP
side: `frontends/gpui/.../drop_zone.rs` had no self-drop guard, so releasing
a drag on a block's OWN drop zone dispatched `move_block{id: S, parent_id:
S}`, asking the engine to make a block its own parent. The dioxus-web copy
had the guard (`info!("drop on self — no-op")`); GPUI never grew one.
COVERAGE primary and unusually crisp — the state is unreachable BY
CONSTRUCTION at two independent points in the keystone:
`crates/holon-integration-tests/src/pbt/transitions/drag_drop_block.rs:70`
filters candidate targets with `*id != source` when generating, and :138
carries the precondition `check(self.source != self.target,
Reason::NoOpParentMove)`, so even a hand-authored self-drop case is rejected
before it runs. ORACLE secondary: no invariant asserts "no dispatched intent
names the same block as subject and parent", so a self-drop reaching the
engine would have been judged only by whatever the engine did with it. FIXED
by moving the guard into the SHARED constructor rather than adding a third
copy: `build_drop_intent` now returns `Option<OperationIntent>` and returns
`None` with an `info!` disclosure when `source_id == target_id`, so every
drop path holds the rule — the two production frontends and
`UserDriver::drop_entity`, which bails with a message naming the refusal.
RESIDUAL GAP: the generator gap is left OPEN on purpose — un-narrowing it
means teaching the reference model that a self-drop is a no-op rather than a
move, which is a reference-model change this lane did not own. The rung that
closes it: drop the `*id != source` filter and turn the `NoOpParentMove`
precondition into an expected-no-op outcome.)

## Missing piece

Unreachable BY CONSTRUCTION at two independent points:
`crates/holon-integration-tests/src/pbt/transitions/drag_drop_block.rs:70`
filters targets with `*id != source` when generating, and :138 carries the
precondition `check(self.source != self.target, Reason::NoOpParentMove)`, so
even a hand-authored self-drop is rejected before it runs. ORACLE secondary:
no invariant asserts "no dispatched intent names the same block as subject
and parent".

## Remedy

FIXED by moving the guard into the SHARED constructor rather than adding a
third copy: `build_drop_intent` returns `Option<OperationIntent>`, `None` +
`info!` when `source_id == target_id`, so both frontends and
`UserDriver::drop_entity` hold it (the driver bails with a message naming
the refusal). Generator gap left OPEN deliberately — un-narrowing needs the
reference model to treat a self-drop as a no-op rather than a move, a change
this lane did not own. Rung: drop the `*id != source` filter, turn
`NoOpParentMove` into an expected-no-op outcome.
