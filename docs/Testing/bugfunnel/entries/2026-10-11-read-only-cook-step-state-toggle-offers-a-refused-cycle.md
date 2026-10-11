---
id: 2026-10-11-read-only-cook-step-state-toggle-offers-a-refused-cycle
date: 2026-10-11
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A step of a read-only `.cook` recipe rendered its `state_toggle` with the
  task-state cycle bound, so a click was an offer the dispatcher's write tier
  then refused (D46); the keystone harness panicked on that refusal.
---
## Bug
Found by the keystone on the places-experiment lane (holon-space/holon-work
#164, workspace places-exp2): `ToggleState` on
`block:Grüne Keystone-Soße.cook::b::0` panicked at
`frontend_slice/components.rs` (`toggle_state_via`) with `block.set_field
failed: … cooklang is a read-only format: … Edit the file on disk to change it.`
The dispatcher already refused the write and disclosed it. The defect is the
offer: D46 rules that an offer the dispatcher refuses is an error. The lane did
not need any of its own changes to reach it. Main reproduces it with
NavigateFocus to the recipe page, then ToggleState
(`target/toggle-ro-red.log`, first half).

## Root cause
The `state_toggle` builder bound `ba.ctx.operations` as given
(crates/holon-frontend/src/shadow_builders/state_toggle.rs). The row's
operations carry every block operation, the tier-judged `set_field` too. Only
`ops_of` withheld those operations on a subject the write tier refuses
(crates/holon-frontend/src/value_fns/ops_of.rs:219). The creation slot does the
same through `offered_creation_parent`.

## Missing piece
`inv-offered-ops-pass-the-write-tier` judged only the `ops_of` offer, not the
toggle's bound op. The ToggleState generator also did not exclude read-only
homes, so the only signal was a harness panic in a rarely drawn case.

## Remedy
- `RenderContext::with_operations` drops the tier-judged operations when the
  write tier refuses the bound row. That is the ONE point every widget reads
  `ctx.operations` from, so the toggle, `editable_text`, `expand_toggle`,
  `rendered_text` and the block chords `render_entity` unions on are all
  offered only what the dispatcher admits. `ops_of` keeps its own filter
  because it offers a subject no bound row names.
- Drag and drop are bound operations too. `draggable` and `drop_zone` bind the
  `move_block` their row is offered, and the GPUI, dioxus-web and headless
  drag/drop paths dispatch only that bound operation; a widget that binds none
  offers no drag or drop. The GPUI board dispatches the `move_block` and
  `set_field` each card binds, and holds still a lane that holds a card it may
  not move (`Sortable` enables drag per lane).
- `BuilderServices::write_tier_admits` is required. `HeadlessBuilderServices`
  (MCP `describe_ui`, the frontend slice) asks the dispatcher's write tier, the
  reference model asks its read-only homes.
- `SutReadOnlyHomes::offered_ops_refused_by_write_tier` judges every operation
  a rendered widget binds on the block, in every region
  (`focus_path::bound_operations_on`), not the Main panel's `state_toggle`
  alone.
- `ToggleState` excludes read-only-homed blocks.
- The GPUI `state_toggle`'s display-only branch names which of the two causes
  it is: a tier-refused row logs at `debug` (the refusal is by design and the
  tier discloses the file), an unwired row still warns.
- Lock: hand-authored case
  `a-read-only-cook-step-offers-no-state-toggle-cycle`, plus the unit lock
  `render_context::tests::a_refused_row_is_offered_none_of_the_operations_the_write_tier_judges`.
  The widened invariant is red before the fix on 156 offers
  (`column` / `editable_text` × 39 block operations × both recipe steps,
  `target/gate-logs/offer-write-tier-red.log`) and green after
  (`inv-offered-ops-pass-the-write-tier=6/6`). It is red again on the
  `drop_zone`'s `move_block` on both recipe steps while drops were unbound
  (`target/gate-logs/offer-write-tier-r2-red.log`), and green once they bind;
  unit lock `render_context::tests::drag_and_drop_bind_the_move_their_row_is_offered`.
