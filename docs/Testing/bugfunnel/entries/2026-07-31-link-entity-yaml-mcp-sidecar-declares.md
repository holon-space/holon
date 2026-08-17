---
id: 2026-07-31-link-entity-yaml-mcp-sidecar-declares
date: 2026-07-31
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A `[[<entity>:<id>]]` link to an entity a YAML MCP sidecar declares NEVER
  resolves — it silently degrades to an unresolved link with no `block_links`
  row and no backlink — for every entity whose name is multi-word.
  `TypeRegistry` is keyed by SQL table name (`EntityName::table_name()`,
  UNDERSCORED; set at `holon-mcp-client/src/mcp_integration.rs:178` via
  `sidecar.prefixed_name(name).table_name()`, stored as `TypeDefinition.name`
  in `mcp_sidecar.rs:441-445`), but a URI scheme is HYPHENATED
  (`EntityName::new` normalizes `_`→`-`, `types.rs:48`). The link classifier
  looked the scheme up verbatim, so `classify("cc-session:abc")` missed the
  `cc_session` key and returned `UnknownScheme`. Single-word entities
  (`person`, `block`, `tag`) are unaffected because the two spellings coincide
  — which is exactly why it escaped: the feature's own keystone probe used
  `person:alice`. Found by adversarial verification of the F1a entity-link
  lane, not by any test.
source_line: 1129
---

## Bug

A `[[<entity>:<id>]]` link to an entity a YAML MCP sidecar declares NEVER
resolves — it silently degrades to an unresolved link with no `block_links`
row and no backlink — for every entity whose name is multi-word.
`TypeRegistry` is keyed by SQL table name (`EntityName::table_name()`,
UNDERSCORED; set at `holon-mcp-client/src/mcp_integration.rs:178` via
`sidecar.prefixed_name(name).table_name()`, stored as `TypeDefinition.name`
in `mcp_sidecar.rs:441-445`), but a URI scheme is HYPHENATED
(`EntityName::new` normalizes `_`→`-`, `types.rs:48`). The link classifier
looked the scheme up verbatim, so `classify("cc-session:abc")` missed the
`cc_session` key and returned `UnknownScheme`. Single-word entities
(`person`, `block`, `tag`) are unaffected because the two spellings coincide
— which is exactly why it escaped: the feature's own keystone probe used
`person:alice`. Found by adversarial verification of the F1a entity-link
lane, not by any test.

## Root cause

secondary COVERAGE: a `[[<entity>:<id>]]` link to any MULTI-WORD
sidecar-declared entity never resolved — it degraded to an unresolved link
with no `block_links` row and no backlink. `TypeRegistry` is keyed by SQL
table name (`EntityName::table_name()`, UNDERSCORED, set at
`holon-mcp-client/src/mcp_integration.rs:178`) while a URI scheme is
HYPHENATED (`EntityName::new` normalizes `_`→`-`), so
`classify("cc-session:abc")` missed the `cc_session` key. Single-word
entities (`person`, `block`, `tag`) are unaffected because the two spellings
coincide — which is exactly why it escaped: the feature's own keystone probe
used `person:alice`. ORACLE, a fixture-choice gap: the keystone DID cover
the whole chain but instantiated it with a scheme that cannot discriminate a
working join from a broken one. Found by adversarial verification, not a
test.)

## Missing piece

The keystone DID cover the entity-link chain end-to-end
(`org_ingest_entity_link_resolves_and_backlinks`) but instantiated it with a
BUILT-IN single-word scheme, so the probe could not distinguish a working
scheme/table-name join from a broken one — a fixture-choice oracle gap, not
a missing property. Missing piece = exercising the chain against an entity
registered through the REAL sidecar path with a MULTI-WORD name.

## Remedy

FIXED 2026-07-31 — lookup folds `-`→`_`
(`holon-profiles/src/type_registry.rs`, `LinkSchemeRegistry for
TypeRegistry`). Red-for-the-right-reason captured at two layers before the
fix: unit/real-registration
(`holon-mcp-client/tests/entity_link_scheme_join.rs`, `got
UnknownScheme("t-widget:abc123")`) and composed keystone
(`structural_pbt.rs::sidecar_entity_link_resolves_through_the_intent_boundary`,
multi-word `t_widget` registered post-boot and driven through
`set_field(content)`). Both green after.
