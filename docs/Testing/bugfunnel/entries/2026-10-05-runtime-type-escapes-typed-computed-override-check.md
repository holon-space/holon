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
log lane-logs/d66-red.log). Known limit: a profile block deleted from the
vault is not observed, so its record stays until the process ends.
