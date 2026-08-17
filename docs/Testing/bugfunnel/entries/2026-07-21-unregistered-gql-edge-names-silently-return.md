---
id: 2026-07-21-unregistered-gql-edge-names-silently-return
date: 2026-07-21
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Unregistered GQL edge names silently return an EMPTY result instead of
  failing loud: any edge name absent from the registry falls through
  `graph_schema.rs`'s EAV path and compiles to a JOIN on the empty EAV `edges`
  table, so a typo'd/unknown edge yields 0 rows with no error (fail-loud
  violation — CLAUDE.md "fail loud, never fake").
source_line: 1074
---

## Bug

Unregistered GQL edge names silently return an EMPTY result instead of
failing loud: any edge name absent from the registry falls through
`graph_schema.rs`'s EAV path and compiles to a JOIN on the empty EAV `edges`
table, so a typo'd/unknown edge yields 0 rows with no error (fail-loud
violation — CLAUDE.md "fail loud, never fake").

## Missing piece

Even if a query with an unregistered edge name were driven, no invariant
asserts unknown-edge → loud error — the silent empty passes every existing
oracle. Secondary COVERAGE: the keystone's alphabet never emits GQL
edge-name queries, so the swallow is also ungeneratable. Remedy:
parse-don't-validate the edge name at compile time — reject an unregistered
edge with an enriched `Err` naming the valid set (kill the EAV silent
fallback) + an oracle/compile test that an unknown edge fails loud.

## Remedy

FIXED 2026-07-21 (holon-side, gql-edge-failloud lane):
`graph_schema::validate_referenced_edges` fails loud at the GQL compile
boundary (`backend_engine.rs::compile_gql`) on any MATCH rel_type absent
from the registry, returning `UnknownEdgeError` naming the offending edge(s)
+ the registered set — killing the silent EAV-default swallow.
EAV-legitimacy analysis: holon populates NO EAV graph tables
(`graph_eav.sql` is never run in prod init; scout-confirmed), so an
unregistered named edge is ALWAYS a typo/unknown, never a valid EAV lookup —
validation is safe (untyped `-[]->` edges still use the default path
unchecked). Same guard woven into the bundled/desk corpus test
(`registration.rs`) as a no-legit-query-breaks safety net (green).
Red-then-green in `graph_schema.rs` (`unknown_edge_fails_loud`). gql-to-sql
FOLLOW-UP (clean home): `GraphSchema::edge_resolver`/`node_resolver` return
`&dyn` infallibly — make them `Result` (or add a strict mode) so unknown
edges fail loud for ALL callers regardless of pre-validation; node-label
silent fallback is the symmetric gap, not yet guarded (PR #2 lint substrate
is the eventual home).
