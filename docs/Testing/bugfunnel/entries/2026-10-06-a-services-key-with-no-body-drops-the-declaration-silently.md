---
id: 2026-10-06-a-services-key-with-no-body-drops-the-declaration-silently
date: 2026-10-06
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A type YAML whose `services:` key has no body (a bare key, or a mis-indented
  body) was accepted with no services, and the mis-indented keys were dropped
  as unknown top-level keys, without an error.
---

## Bug
Found by the adversarial verifier of type-services C1 (lane report
`lane-logs/ts-c1-verify.md`, probe log `lane-logs/ts-c1-verify-probe.log`),
before landing. The C1 claim was "malformed `services:` declarations are
refused by name at registration". A bare `services:` parsed as `None`, so
`title` was not required. With a mis-indented body, `title:` and
`searchable:` became top-level keys of `TypeDefinition`, which accepted
unknown keys, so the whole declaration vanished.

The same verifier found further silent acceptances in the same lane, fixed
together:
- a service could name the engine-owned overflow bag (`value_kind:
  overflow_properties`) or a transient field;
- `hierarchy.parent` could be the type's own primary key, and
  `hierarchy.order` a TEXT field;
- an ill-typed field reference was refused by its value, not by its key;
- a declaration that the registry refuses through `declare_type` had already
  created its Turso table and matview.

## Root cause
`TypeDefinition.services` was `Option<TypeServices>` with `#[serde(default)]`
(crates/holon-api/src/entity.rs), and serde maps a YAML null to `None`, so a
present key with no value was the same as an absent key. `TypeDefinition` had
no `deny_unknown_fields`. The optional keys inside `services:` had the same
null-to-absent shape. `TypeServices::check_fields`
(crates/holon-api/src/type_services.rs) read only field existence and SQL type.
`declare_type` (crates/holon/src/core/type_declaration.rs) ran the Turso DDL
before `TypeRegistry::register`.

## Missing piece
The C1 tests (crates/holon-profiles/tests/type_services_declaration.rs) never
generated a key present with a null value, a mis-indented body, or an unknown
top-level key, and no test drove a registry refusal through `declare_type`.

## Remedy
`services:` and each optional key inside it refuse a null value by key name;
`TypeDefinition` denies unknown keys; field references parse with their key in
the error; `check_fields` refuses engine-owned and transient fields, a
self-parent and a non-numeric order. `declare_type` runs
`TypeRegistry::check` before any DDL. Pinned by the tests in
`type_services_declaration.rs` and
`a_declaration_the_registry_refuses_creates_no_turso_artifacts`.
