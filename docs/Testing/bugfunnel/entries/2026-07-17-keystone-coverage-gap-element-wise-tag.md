---
id: 2026-07-17-keystone-coverage-gap-element-wise-tag
date: 2026-07-17
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Keystone coverage gap for element-wise tag ops: the new
  `add_tag`/`remove_tag` block ops (idempotent, invertible,
  Page-nesting-guarded) are proven by per-provider unit tests + a
  byte-identical catalog-parity certificate, but the ONE composed keystone PBT
  (`general_e2e_composed_pbt`) has NO `add_tag`/`remove_tag` transition, so
  element-wise tag mutation is not exercised end-to-end (generator → SUT →
  reference-state → correspondence) by the keystone. A NON-Page tag transition
  was deliberately NOT added this workstream: (a) the keystone base is
  currently RED (display-placement Phase-1a WIP; the F8-unmasked
  `inv-history-records-all-creates` creation-slot gap was RESOLVED 2026-07-23,
  see row above), so a new transition would land on an unstable base and its
  own reddening would be indistinguishable from the baseline; (b) Page-tag
  transitions are FORBIDDEN by generator guarantee R8 (pages are seed-only),
  so only non-Page adds/removes are generatable, needing a new reference-state
  tag-set model + a `tags`-set correspondence to be non-vacuous.
source_line: 999
---

## Bug

Keystone coverage gap for element-wise tag ops: the new
`add_tag`/`remove_tag` block ops (idempotent, invertible,
Page-nesting-guarded) are proven by per-provider unit tests + a
byte-identical catalog-parity certificate, but the ONE composed keystone PBT
(`general_e2e_composed_pbt`) has NO `add_tag`/`remove_tag` transition, so
element-wise tag mutation is not exercised end-to-end (generator → SUT →
reference-state → correspondence) by the keystone. A NON-Page tag transition
was deliberately NOT added this workstream: (a) the keystone base is
currently RED (display-placement Phase-1a WIP; the F8-unmasked
`inv-history-records-all-creates` creation-slot gap was RESOLVED 2026-07-23,
see row above), so a new transition would land on an unstable base and its
own reddening would be indistinguishable from the baseline; (b) Page-tag
transitions are FORBIDDEN by generator guarantee R8 (pages are seed-only),
so only non-Page adds/removes are generatable, needing a new reference-state
tag-set model + a `tags`-set correspondence to be non-vacuous.

## Missing piece

no keystone transition drives add_tag/remove_tag; adding one needs a
reference-state tag-set model + tags correspondence and a green keystone
base to land against

## Remedy

OPEN — deferred; add a non-Page `add_tag`/`remove_tag` transition once the
keystone base is green (creation-slot history gap RESOLVED 2026-07-23;
display-placement remaining)
