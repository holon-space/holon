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
EAV-legitimacy analysis: holon WRITES no EAV graph tables, so an
unregistered named edge is ALWAYS a typo/unknown, never a valid EAV lookup —
validation is safe (untyped `-[]->` edges still use the default path
unchecked — no longer true as of BG-5; see below). Same guard woven into the bundled/desk corpus test
(`registration.rs`) as a no-legit-query-breaks safety net (green).
Red-then-green in `graph_schema.rs` (`unknown_edge_fails_loud`). gql-to-sql
FOLLOW-UP (clean home): `GraphSchema::edge_resolver`/`node_resolver` return
`&dyn` infallibly — make them `Result` (or add a strict mode) so unknown
edges fail loud for ALL callers regardless of pre-validation; node-label
silent fallback is the symmetric gap, not yet guarded (PR #2 lint substrate
is the eventual home).

### 2 of 3 residuals CLOSED under BG-5 (2026-08-31); the third stays OPEN

The two shapes this entry left unguarded — the untyped `-[]->` edge and the
symmetric node-label fallback — are now refused by name.
`validate_referenced_edges` became `validate_typed_shape`
(`crates/holon-turso/src/graph_schema.rs`), returning `UntypedGqlShape`
(`UnknownEdge` / `UntypedEdge` / `UnknownNodeLabel` / `UnlabelledNode`). The `GraphSchema`
`default_node_resolver`/`default_edge_resolver` no longer name the EAV tables;
`gql-transform` requires the fields, so they point at a table that exists in no
database, making any residual path fail identically on fresh and legacy DBs.
Red-then-green: `untyped_edge_fails_loud`, `unknown_node_label_fails_loud`.

The THIRD residual listed above stays **OPEN**: `GraphSchema::edge_resolver` /
`node_resolver` still return `&dyn` infallibly, so unknown shapes fail loud only
for callers that pre-validate. `validate_typed_shape` walks MATCH clauses only,
which is exactly what that residual predicts — `CREATE (a:not_a_registered_label)`
is NOT validated and reaches the default resolver at execution time. It now fails
uniformly and loudly (`no such table: __holon_no_typed_resolver__`) instead of
inserting into the legacy EAV tables, so this is strictly better than the base
behaviour, but the shape is still reachable. Extending validation to CREATE (and
the junk-rows-written-before-failure question it raises) is a follow-up lane.

Two premises in the text above were WRONG and are corrected here (found by
adversarial verification, not by a test):

- "`graph_eav.sql` is never run in prod init" — false. The DDL ran on EVERY
  boot (`all_schema_roots()` → `DbReady<GraphEavSchema>`). The tables were
  created-and-empty, not absent, which is exactly why the fallthrough returned
  0 rows instead of erroring. BG-5 deleted the schema entirely.
- Only the WRITE side was ever confirmed absent. A source grep cannot see these
  readers at all: the SQL naming `nodes`/`edges` is generated inside the
  external `gql-transform` crate, so no literal `FROM nodes` appears anywhere in
  `crates/`. The reader must be found by compiling a query through the
  production `GraphSchema`, not by grepping.
