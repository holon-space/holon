---
id: 2026-11-22-capability-supports-offers-refused-property-keys
date: 2026-11-22
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  `supports(PropertyKey)` answered Offered for an engine-owned key the format
  refuses, and a declaration refusal built from a registry error named no type
  and no field.
---

## Bug
Found by the per-crate code review of `holon-capability`; no production caller
asks `Feature::PropertyKey` yet, so nothing user-visible broke.

## Root cause
- `crates/holon-capability/src/supports.rs`: the `PropertyKey` arm consulted
  the reserved-prefix and owned lists but not `is_engine_owned`, and the
  `ReorderSiblings` arm ended in `_ => Offered`.
- `crates/holon-capability/src/declaration.rs`: `unoffered_reason` mapped a
  registry error to a `HomeRefusal` with empty `type_name` and `field`. Type
  admission checks every declared home first, so only a re-home onto an
  unchecked destination reaches it.
- `crates/holon-capability/src/clause.rs`: `site_of(..).unwrap_or_default()`
  hid the impossible "deferred clause without a site".

## Missing piece
`supports` had no test and no generator: nothing drew a property-key list set
and compared the offer with the write boundary's refusal set.

## Remedy
`supports(PropertyKey)` refuses engine-owned keys. A crate-level property
(`supports.rs` tests, generators in `fixture.rs`) asserts offer iff no list
refuses the key. The refusal is built where the type and field are known, and
one `deferral_of` lookup replaces `layer_of` plus `site_of`.
