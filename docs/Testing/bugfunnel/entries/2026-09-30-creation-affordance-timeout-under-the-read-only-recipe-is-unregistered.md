---
id: 2026-09-30-creation-affordance-timeout-under-the-read-only-recipe-is-unregistered
date: 2026-09-30
gap: FALSE-ALARM
secondary: null
status: OPEN
summary: >-
  The keystone's creation-affordance wait ("birth of <block> under <recipe
  page> did not land within 3s") fires on a create the read-only write tier
  correctly refuses, and no registry row matches it, so every run that draws
  it is classified as a NOVEL red.
---

## Bug
Seen as the minimal failing case of plain `just pbt general 12` runs twice:
the decision Inc 7 round-3 lane run 1 (`lane-logs/inc7r3-pbt1.log:2102`) and
the round-3 verifier's run 1 (`lane-logs/inc7r3v-g4.log:2113`, DEFECT 7 of
`lane-logs/inc7r3v-verify.md`). Both runs' primary panic was a registered
known red (`cooklang-read-only-write-refusal-any-op`); the shrink tail ended
on

`[SutBlockCreate::apply_create_under_focus] creation-affordance birth of
block:<uuid> under block:5f54ed1a-400a-8b5e-1e0e-9b62ad8370cd did not land
within 3s — the block.create dispatched by the focus edge never produced a row`

`block:5f54ed1a-…` is the page of the keystone's read-only recipe
(`keystone-recipe.cook`). `grep -c "did not land within"
docs/Testing/KeystoneKnownReds.md` was 0.

## Root cause
The same mechanism as
`2026-09-16-keystone-create-under-a-read-only-focus-root-panics`, which
recorded this text as its second symptom: `NavigateFocus` can seat the focus
on the recipe page, `ApplyCreateUnderFocus` then draws a create there, the
write-tier gate refuses it (correct, Model.md invariant 14), and the
creation-affordance driver waits 3 s for a row that cannot appear
(`crates/holon-frontend/src/user_driver.rs:758`). No product defect: the
refusal is the designed behaviour. The escape recorded here is the
classification: the signature matches no `Match pattern`, so
`scripts/keystone-known-reds.sh` reports it as NOVEL on every run that draws
it, and a lane cannot tell it from a regression without re-deriving this
triage.

## Missing piece
A registry row for the signature, and (the fix proper, owned by the
2026-09-16 entry) a refusal-shaped expectation for a create under a
read-only-homed focus root.

## Remedy
Proposed row `creation-affordance-read-only-focus-root` in
`docs/Testing/KeystoneKnownReds.md`, status `proposed` (it does not classify)
until Martin ratifies it (D145.a). The harness fix stays with
`2026-09-16-keystone-create-under-a-read-only-focus-root-panics`.
