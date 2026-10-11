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
- The `state_toggle` builder drops the tier-judged operations when the write
  tier refuses the row, so the toggle shows the state and binds no cycle.
- `SutReadOnlyHomes::offered_ops_refused_by_write_tier` also judges the Main
  panel's `state_toggle` cycle intent.
- `ToggleState` excludes read-only-homed blocks.
- Lock: hand-authored case
  `a-read-only-cook-step-offers-no-state-toggle-cycle`. It is red before the fix
  on `inv-offered-ops-pass-the-write-tier` (`state_toggle set_field` on both
  recipe steps) and green after (`target/toggle-ro-green.log`).
- Still open: the other widgets of a read-only row still carry every block
  operation in their `operations`.
