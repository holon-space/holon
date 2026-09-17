---
id: 2026-08-24-drawer-open-matches-ref-flake
date: 2026-08-24
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  `inv-drawer-open-matches-ref` intermittently mismatched in a keystone draw.
  ROOT-CAUSED 2026-09-17: the SUT-side drawer-toggle gesture never ran. The
  reference flipped its `drawer_open` bit while the headless medium wrote
  nothing — `drawer_toggle::<block_id>` is an element only GPUI registers, so
  the headless click unwrapped to a bare click on the sidebar block, which
  resolves the row's `navigation_focus` and degrades to bare focus. The
  invariant was RIGHT; the medium could not perform the gesture. Fixed in the
  medium, never in the invariant.
---

## Bug

The `inv-drawer-open-matches-ref` invariant reported a mismatch between the
rendered drawer-open state and the reference model's expected drawer-open
state during a keystone run on 2026-08-24. No detail of the mismatched case
(which block, which drawer, the diff) survived a session restart — only the
orchestrator's adjudication note (signature name, method, verdict, date) is
available for this entry.

`docs/Testing/bugfunnel/entries/2026-08-16-web-builder-ignores-open-closed-state.md`
is where `inv-drawer-open-matches-ref` was introduced
(`crates/holon-integration-tests/src/pbt/invariants/bodies/drawer_open_matches_ref.rs`),
proven red-for-the-right-reason then green, and it carries an explicit
disclosed residual about unverified PIXEL rendering. This entry's sighting is
in the snapshot-layer comparison the invariant itself performs, which that
origin entry treats as closed.

## Root cause

**FOUND 2026-09-17 — the reference model and the invariant were both right; the
headless SUT medium could not perform the gesture the transition models. The
`missing piece` this entry asked for (the specific case and the before/after
values) is no longer needed: the defect is deterministic, not a race.**

`ToggleDrawer` is a member of the composed alphabet (`E2ETransition::ToggleDrawer`,
cap-gated on `SutBlockInteract`, which the headless composed `CapMap` provides
via `DriverInputComponent::with_input_headless`). Its two halves disagree:

- `apply_to_ref` flips `ReferenceState.ui.tab.drawer_open[<schemed id>]`
  (`RefToggleMut::toggle_drawer`).
- `apply_to_sut` clicks the handle `drawer_toggle::<block_id>`
  (`holon_layout_testing::transitions::toggle_drawer`). Only ONE production
  renderer ever registers that element:
  `frontends/gpui/src/render/builders/drawer.rs` (`drawer_toggle_widget`). The
  headless medium has no such widget, so
  `DriverInputComponent::click_at_element` unwrapped `<kind>::<uri>` to a bare
  `click_entity(block:default-left-sidebar)`, which
  `ReactiveEngineDriver::click_entity_with_modifiers` resolves against the
  sidebar panel — landing on the sidebar row's `navigation_focus` intent and
  otherwise degrading to bare focus. Nothing on that path writes widget state:
  `grep -rn set_widget_open` reaches `FrontendSession` only from GPUI code.

So the reference's bit flipped and the SUT's `widget_open` store was never
written, leaving the SUT on its mode default (`Shrink` ⇒ `default_open() == true`)
while the reference said closed. Measured in the repro:
`[inv-sql-budget] ToggleDrawer: reads=9 (dedup 8)/15 writes=0/0`.

This also closes the entry's two recorded hypotheses — neither holds:

1. the ENVIRONMENT settle-timing race is REFUTED: the mismatch is not
   transient, it is the permanent post-toggle state, and it reproduces
   deterministically with a single transition and no RNG;
2. the ORACLE timing-sensitivity hypothesis is REFUTED: the invariant samples
   settled state and is correct about it. Hence `secondary: ORACLE` is cleared.

`gap: ENVIRONMENT` is retained (not re-classed to ORACLE): the environment
could not reach a production affordance, which is exactly the escape this
ledger's ENVIRONMENT class exists for.

## Missing piece

**CLOSED for the headless half.** The invariant's failure payload now names
both values (`rendered open=true but reference says open=false`), the cause is
deterministic, and the regression is locked by a hand-authored keystone case
(`drawer-toggle-click-leaves-the-sidebar-open-in-the-sut` in
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`,
one transition, no RNG).

**STILL OPEN / partially covered (disclosed):** the windowed half. The GPUI
windowed loop and the TUI PBT register the same `DriverInputComponent`, so this
same silent mis-drive applied there; the shared fix covers them, and six
post-fix TUI runs produced zero drawer divergences — but neither windowed slice
draws `ToggleDrawer` in its current alphabet, so no windowed arm yet exercises
the fixed branch itself. The registry row is `fixed-pending-soak` and a
recurrence classifies as NOVEL.

## Remedy

**FIXED 2026-09-17.** Three changes, all in the medium:

1. `BuilderServices::toggle_drawer(id, mode)` (`crates/holon-frontend/src/reactive.rs`)
   — the flip is read-then-inverted off `drawer_open`, single-sourced, and now
   also the body of GPUI's `finalize_sidebar_resize`
   (`frontends/gpui/src/render/builders/drawer.rs`) so the production handler
   and the PBT drivers cannot drift.
2. `DriverInputComponent::click_at_element`
   (`crates/holon-integration-tests/src/pbt/driver_input.rs`) routes a
   `drawer_toggle::<id>` handle to the new
   `DriverInputComponent::toggle_rendered_drawer`, which performs exactly that
   write against the RENDERED drawer mode
   (`holon_frontend::focus_path::rendered_drawer_mode`) — the same mode the
   production handler reads off the drawer it hit — and fails loud when the
   block renders no drawer node (naming the drawers that ARE rendered).
3. `focus_path::{rendered_drawer_mode, rendered_drawer_ids}` walk the resolved
   tree for the drawer node.

Red-first proof: the new hand-authored case was RED at base `bd8b719a` with
this entry's signature (`lane-logs/repro-1.log:101`) and GREEN after the fix
(`lane-logs/green-1-case.log:54`). Post-fix: `just hand-authored` 87/87 green;
`HOLON_PBT_FORCE_FULL=1 HOLON_PBT_WEIGHTS=ToggleDrawer:200 just pbt general 3`
green with the transition actually drawn 8 times and
`inv-drawer-open-matches-ref` engaging 17/17, 2/2, 6/6;
`HOLON_PBT_FORCE_FULL=1 just pbt general 8` produced zero drawer signatures.
