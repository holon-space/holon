---
id: 2026-10-05-runtime-type-escapes-typed-computed-override-check
date: 2026-10-05
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A type registered at runtime with a typed computed field escaped the D66.a
  refusal, so an already-loaded vault profile could replace that field with a
  Rhai script on the live seat alone.
---

## Bug
Found by the verifier of lane d45-followups round 1. `profile_load_check`
(crates/holon-profiles/src/type_registry.rs) copied the type map once, at boot.
A type registered afterwards (`TypeRegistry::register`, reached by
`declare_type` and the declare_type op/MCP) could add a typed computed field
that a loaded vault profile already redeclared. `merge_profile` then let the
Rhai field win on the live seat only, which D66.a forbids.

## Root cause
Two staleness shapes. The check closure read a boot snapshot. And nothing
remembered what loaded profiles declare, so a type that arrived second was
never compared with them. The older `check_profile_scope` check shared the
snapshot: a profile loaded or edited after a type was declared was checked
against a map without that type, and refused wrongly.

## Missing piece
The D66.a test loaded the profile against the types present at boot and never
registered a type afterwards. No keystone generator declares types at runtime
together with vault profiles.

## Remedy
The check closure reads the live type map, and records each accepted profile's
computed field names (a refused load releases its record). `register` refuses a
type whose typed computed field a recorded profile redeclares, with
`TypedComputedFieldOverride`. Tests: the three added to
crates/holon-profiles/tests/profile_typed_computed_override_refusal.rs (red
log lane-logs/d66-red.log). A profile block that leaves the vault releases its record:
`create_profile_resolver` (crates/holon/src/di/registration.rs) watches the
profile mirror's `MapDiff::Remove` and calls `TypeRegistry::profile_release`.
Test: crates/holon-app/tests/deleted_vault_profile_releases_its_claim.rs (red
log lane-logs/d66b-red.log: the type stayed refused after the profile's
document was deleted).

A refused profile EDIT keeps the older version's record: the mirror drops the
refused row and the older version stays applied (measured by
`live_data::tests::an_unparseable_update_keeps_the_earlier_version`, log
lane-logs/d66c-measure.log), so the load check no longer releases a record on
refusal. Test: `a_refused_edit_leaves_the_live_profiles_claim_in_force`, run
inside `a_vault_profile_claim_follows_the_live_version_of_its_block` (red log
lane-logs/d66c-red.log: the type registered although the older version was still
applied).
