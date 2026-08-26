---
id: 2026-08-26-switch-view-mode-silent-no-op
date: 2026-08-26
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A headless `SwitchViewMode` step accepted any (block, mode) pair, clicked a
  `vms_button::` handle nothing answers, and passed having changed nothing.
---

## Bug

Found by agent exploration while auditing the fixture step vocabulary (D35.a
item 3, lane `fix-oracle-gaps`), not by any test. A `.feature` scenario may
write `When I switch block "block:c1" to view mode "table_view"`. In the
headless composed medium that step is accepted, executed, and observably does
nothing: no mode changes, no error is raised, and the scenario passes. A
fixture author reading a green run would conclude the mode switch works.

## Root cause

Three layers each declined to know the truth:

- `SwitchViewMode::preconditions`
  (`crates/holon-layout-testing/src/transitions/switch_view_mode.rs`) returned
  `Good(())` for every state, so no (block, mode) pair was ever refused —
  while the sibling `weighted_generator` in the same file correctly fails
  `NoSwitchableHandles` on the same state.
- `apply_to_ref` is empty (the ref state models no VMS mode), so the oracle
  half of the transition cannot disagree with the SUT half.
- `DriverInputComponent::click_at_element`
  (`crates/holon-integration-tests/src/pbt/driver_input.rs:569`) parses the
  handle's `<kind>::<target>` prefix, branches only on `expand_toggle`, and
  otherwise unwraps the remainder to an `EntityUri`. For
  `vms_button::block:c1::table_view` that yields the URI `block:c1::table_view`
  — syntactically valid, semantically a ghost. `ReactiveEngineDriver::
  click_entity_with_modifiers` then polls two seconds for an entity that never
  appears, logs a `warn!`, and degrades to bare focus on the ghost URI,
  returning `Ok(())`.

The headless medium genuinely cannot perform this switch: in GPUI the mode
change is a view-local closure over a `Mutable` plus a `ReactiveView::
set_template` call (`frontends/gpui/src/render/builders/view_mode_switcher.rs`),
never an `OperationIntent`, so there is nothing in the resolved view tree for a
headless driver to dispatch. Making it "work" headlessly would mean
re-implementing input outside the production driver — the honesty gate the
driver rung exists to hold. The correct behaviour is therefore a loud refusal,
not a synthesized switch.

## Missing piece

No oracle: the transition could execute end to end with every assertion
satisfied, because neither a precondition, a ref-state delta, nor an invariant
observes view mode at all. Secondarily a coverage gap — `LayoutRefState::
switchable_handles` for `ReferenceState`
(`crates/holon-integration-tests/src/pbt/layout_bridge.rs:74`) is hardcoded
empty, so the keystone generator can never produce a `SwitchViewMode` and only
a hand-authored fixture reaches the code at all.

## Remedy

- `preconditions` now consults `switchable_handles()`: an unlisted block fails
  `NoModeSwitchableSurface`, a listed block asked for a mode it does not offer
  fails `ModeNotOfferedByBlock`. Under strict fixture replay both are a hard
  panic.
- `click_at_element` (`crates/holon-integration-tests/src/pbt/driver_input.rs`)
  refuses a `vms_button::` handle by name instead of laundering it into an
  entity click. This guard is independent of the precondition on purpose: if
  the `switchable_handles` TODO is ever implemented, the precondition starts
  passing and this is what still refuses an unresolvable handle.
- `replay_steps`' precondition assertion now prints the `Reason`s, so a refusal
  names why rather than only which transition.

Pinned by `tests/fixtures/_gherkin_negative/switch_view_mode_no_surface.feature`
replayed under `#[should_panic]` in
`tests/catalog_suite/gherkin_negative_replay.rs`.

- Red before the fix: `target/lane-logs/item1-red.log` — the control did NOT
  panic; the run logged `click_entity: entity never appeared in the resolved
  tree; the click binds no intent and degrades to bare focus` for
  `block:c1::table_view` and the scenario reported OK.
- Green after: `target/lane-logs/item1-green-verbose.log` — `step 0:
  preconditions FAILED for SwitchViewMode (NoModeSwitchableSurface)`, and no
  click is attempted.
