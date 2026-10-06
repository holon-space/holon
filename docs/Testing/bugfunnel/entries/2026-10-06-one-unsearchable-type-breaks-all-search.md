---
id: 2026-10-06-one-unsearchable-type-breaks-all-search
date: 2026-10-06
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  One searchable type whose branch cannot compile (a non-persistent searchable
  field, or a type whose table search guessed wrong) made quick-open and `[[`
  fail for every type.
---

## Bug
Found by the adversarial verifier of type-services C2 (lane-logs/ts-c2-verify.md),
outside an automated test. Search is one `UNION ALL` statement. Two reachable
states produced a branch that does not compile, and then every search returned
an error: (a) `services.searchable` naming a `historical` or `computed` field,
which `declare_type` admitted; (b) an MCP `create_entity_type` type with
`services`, whose table is `"{name}"` while search read `"{name}_raw"`.

## Root cause
`authored_field` (crates/holon-api/src/type_services.rs) refused only
missing, engine-owned and transient fields. `branch()`
(crates/holon/src/api/entity_search.rs) read `TursoAdapter::raw_table_name`,
which exists only for adapter-stored types. MCP `create_entity_type`
(frontends/mcp/src/tools.rs) registered the type before it created the table
and kept the registration when the DDL failed.

## Missing piece
No keystone transition declares a type with a non-persistent searchable field,
and no test creates a searchable type through MCP and then searches.

## Remedy
Every non-persistent lifetime is refused by name at declaration. Search keeps
reading the adapter's raw table (`TursoAdapter::raw_table_name`), and MCP
`create_entity_type` refuses a type that declares `services`, by name. MCP
checks, creates the table, then registers. Tests: crates/holon-profiles/tests/type_services_declaration.rs,
crates/holon-integration-tests/src/pbt/transitions/register_entity_scheme.rs.
