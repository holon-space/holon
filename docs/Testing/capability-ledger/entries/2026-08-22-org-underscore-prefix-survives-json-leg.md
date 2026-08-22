---
id: 2026-08-22-org-underscore-prefix-survives-json-leg
date: 2026-08-22
profile: org
axis: property_keys
clause: reserved_prefixes
leg: org_properties_json
status: OPEN
summary: >-
  org declares `_` a reserved prefix, but the org_properties JSON leg carries
  `_`-prefixed keys intact — the erasure belongs to the flat leg, not to the
  format
---

## What the certifier saw

`crates/holon-org-format/tests/profile_certification.rs`, Increment 2b.1:

```
TIGHTENING [org] axis=property_keys leg=org_properties_json key="_underscored"
  sent=String("carried"): declared under a reserved PREFIX, but this leg carried
  it — the erasure is not a property of the format, only of this leg
```

## Why it happens

The `_`-prefix filter lives in `OrgBlockExt::drawer_properties()`
(`crates/holon-org-format/src/models.rs:886`, `:894`, `:909`). The renderer
calls it only to RECONSTRUCT a drawer, and only when the block carries no
`org_properties` JSON (`crates/holon-org-format/src/org_renderer.rs:393`). A
block that already has `org_properties` set renders its drawer verbatim from
that JSON (`models.rs:169-206`), and the `_`-prefixed key reaches disk:

```
"#+TITLE: Certify\n* Probe headline\n:PROPERTIES:\n:ID: b1\n:_underscored: carried\n:END:\n"
```

## Why the profile still declares it reserved

Declaring `_` unreserved would make the FLAT leg go red, and that leg is the
one the production write-back path uses for a block coming out of the store.
Declaring it reserved is the conservative, honest choice for the format as a
whole; this entry records that the reservation is stricter than one of the two
legs needs.

## What it prompts

Axis 3 currently reserves prefixes per FORMAT. This is the first evidence that a
reservation can be per-CARRIER. Decide in 2b.2 whether `reserved_prefixes`
grows a per-leg qualifier, or whether the two legs should be reconciled so the
format has one answer. Reconciling is the better outcome if it is cheap: two
legs that disagree about what reaches disk is the same class of hazard as the
cross-format `_logseq_raw/` finding.
