---
id: 2026-10-13-toggle-state-on-a-decision-block-is-never-applicable
date: 2026-10-13
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The composed keystone drove `ToggleState` on a `decision` block, where the
  shape gate's rule B4 refuses every keyword the production cycle ring can
  write, so the harness panicked on a refusal the product owes the user.
---

## Bug

holon-work#183: the composed keystone dies on the first draw on main
`53c5b430` with

```
[toggle_state] click #1 failed for block:kd-1: dispatch_intent_sync: block.set_field failed:
Operation 'set_field' on entity 'block' failed: refused: after this write the `decision` block
block:kd-1 breaks rule B4: B4: decision block:kd-1 has keyword Some("TODO");
a decision is `?`, DONE or CANCELLED
```

Found by the keystone itself (lane tri-b triage of #183, part of #163).
Reproduced deterministically through the hand-authored harness — see
`docs/Testing/HandAuthoredRegressions.md` — with a two-step case
(`NavigateFocus(main, block:decision-page)`, `ToggleState(block:kd-1, TODO)`):
red log `target/tri-b-red.txt`.

## Root cause

A `decision` block's task keyword IS its status. Rule B4 admits only `?`,
`DONE` and `CANCELLED` there (`crates/holon-api/src/decision_block.rs:74-130`,
`169`), and `DecisionShape` states that the keyword is written by the decision's
own op, not as text (`crates/holon-core/src/shape_gate/decision.rs`).

A `state_toggle` click walks the OWNING DOCUMENT's `#+TODO:` ring, which for a
document that declares no vocabulary is `["", "TODO", "DOING", "DONE"]`
(`crates/holon/src/core/task_keyword_cycle.rs`, `NATIVE_RING`). `?` is not in
that ring, so the unknown current keyword falls back to index 0 and the first
click writes `TODO` — which B4 refuses. Hence:

- targets `Clear`/`TODO`/`DOING` are refused outright at click #1 (B4);
- target `DONE` is unreachable too: production must walk `TODO` and `DOING`
  first, each refused, so the click sequence can never arrive.

The product is right on both legs: the write is refused and the refusal is
disclosed. What was wrong is the model: `ToggleState::preconditions`
(`crates/holon-integration-tests/src/pbt/transitions/toggle_state.rs`) had no
decision check, so the generator offered a gesture that can never land, and the
driver (`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:4553`)
panicked on the refusal.

Tolerating the refusal in the driver would NOT have been enough: for the `DONE`
target the reference model's own `broken_shape` arms rule `DC4` (a ruling needs
`chosen`, `crates/holon-api/src/decision.rs:721,764`) while production reports
`B4`, so `shape_state::is_expected_refusal` — which matches on the exact rule
code — could not match.

## Missing piece

`ToggleState`'s preconditions excluded read-only-homed blocks, layout
headlines and custom block profiles — every other place where production paints
no usable `state_toggle` cycle — but not the one block class whose keyword the
cycle may not write at all.

## Remedy

One more precondition check: `ToggleState` is not applicable to a block tagged
`decision` (`Reason::StateToggleNotApplicable`, the reason the three sibling
exclusions already use). Locked by the unit tooth
`no_cycle_target_is_offered_on_a_decision_block` in `toggle_state.rs`, which
asserts kd-1 IS in the generator's candidate pool (non-vacuity) and that no
`CycleTarget` passes the preconditions. Green log `target/tri-b-green.txt`.

The hand-authored repro case is deliberately NOT committed: that harness applies
transitions without precondition filtering
(`crates/holon-integration-tests/src/pbt/hand_authored.rs`), so a committed row
would stay red after a generator-side fix.

Follow-up, NOT fixed here (new behavior, needs the `holon-feature` route and
Martin's ruling): production still paints a live `state_toggle` on a decision
row whose clicks can never advance its status — a visibly-disclosed dead
affordance. Making the cycle ring shape-aware (`?` → `DONE` → `CANCELLED` on a
decision) is a feature decision.
