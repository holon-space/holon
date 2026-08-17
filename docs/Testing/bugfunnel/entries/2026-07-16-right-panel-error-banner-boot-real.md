---
id: 2026-07-16-right-panel-error-banner-boot-real
date: 2026-07-16
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Right-panel error banner at boot on real vault: `GQL transform error:
  UnknownProperty { entity: "focus_root", property: "added_ts" }` — a shipped
  desk/right-panel query references a property the entity doesn't have (loud,
  good; but permanently broken panel)
source_line: 832
---

## Bug

Right-panel error banner at boot on real vault: `GQL transform error:
UnknownProperty { entity: "focus_root", property: "added_ts" }` — a shipped
desk/right-panel query references a property the entity doesn't have (loud,
good; but permanently broken panel)

## Missing piece

no smoke test executes every bundled/seeded query (same gap as the
2026-07-10 Journals-page-query row)

## Remedy

FIXED 2026-07-17 — root cause: the `focus_root` GQL node (schema_modules.rs)
declared only region/block_id/root_id, but the `focus_roots` matview also
projects `added_ts`/`history_id`, and the right-sidebar query ORDERs BY
`fr.added_ts`. Fix: `focus_root` node now declares every matview column
(dropped the phantom `block_id`, which the matview never has); also exposed
the physical `block.sort_key` column on the block GQL node (registration.rs)
since the same query ORDERs BY `d.sort_key` (`RETURN d` projects `node.*`,
so this only enables property refs). Gap class closed:
`bundled_gql_query_smoke` (holon di::registration) builds the production
GraphSchema and compiles the desk panel GQL queries + any bundled
`holon_gql` asset block against it, failing on UnknownProperty-class errors.
The keystone PBT missed this because its `to_gql` drops the ORDER BY clause.
