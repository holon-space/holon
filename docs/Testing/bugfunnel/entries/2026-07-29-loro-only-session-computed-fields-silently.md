---
id: 2026-07-29-loro-only-session-computed-fields-silently
date: 2026-07-29
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A Loro-only session's computed fields are silently `Null`.
  `build_turso_free_profile_resolver`
  (`crates/holon/src/api/loro_ui_watcher.rs`) constructed its
  `ProfileResolver` with `LiveEntities::new()`, so under
  `StorageSelector::LoroMemory` the bundled block profile's `has_query_source`
  (`query_source(id) != () && rule_sibling(id) == ()`) and `is_program`
  (`rule_sibling(parent_id)`) evaluated on a Rhai engine with neither lookup
  registered — both land at `Null`, so the `query_block` and `rule_card`
  variants can never be selected and a query page renders as plain content.
  The Turso arm fed the same two entities from CDC matviews
  (`di/registration.rs`); the no-Turso arm simply had no seat. Exact mirror
  image of the 2026-07-28 row below, which fixed the *oracle*-side engine —
  the same defect on the third (Loro) arm survived it. Found by code
  inspection during a wiring audit, not by any test.
source_line: 789
---

## Bug

A Loro-only session's computed fields are silently `Null`.
`build_turso_free_profile_resolver`
(`crates/holon/src/api/loro_ui_watcher.rs`) constructed its
`ProfileResolver` with `LiveEntities::new()`, so under
`StorageSelector::LoroMemory` the bundled block profile's `has_query_source`
(`query_source(id) != () && rule_sibling(id) == ()`) and `is_program`
(`rule_sibling(parent_id)`) evaluated on a Rhai engine with neither lookup
registered — both land at `Null`, so the `query_block` and `rule_card`
variants can never be selected and a query page renders as plain content.
The Turso arm fed the same two entities from CDC matviews
(`di/registration.rs`); the no-Turso arm simply had no seat. Exact mirror
image of the 2026-07-28 row below, which fixed the *oracle*-side engine —
the same defect on the third (Loro) arm survived it. Found by code
inspection during a wiring audit, not by any test.

## Root cause

`build_turso_free_profile_resolver` passed `LiveEntities::new()`, so every
`StorageSelector::LoroMemory` session evaluated the bundled block profile's
`has_query_source` / `is_program` against a Rhai engine carrying NO
`query_source` / `rule_sibling` lookup — both fields silently `Null`,
query-page and rule-machinery routing gone, in the one wiring that has no
Turso CDC to feed them. The no-Turso arm DOES run under test
(`BlockQueryFrontendComponent`), but nothing ever asserted a computed field
there, so the Null was rendered and never flagged. Fixed with the
`LiveEntitySpec` single seat + a source-backed refresh; red-first in
`tests/loro_live_entity_wiring.rs`.)

## Missing piece

The no-Turso wiring is not untested — `BlockQueryFrontendComponent` boots
the real `from_block_query_source` stack — but every assertion there is
about the RENDER KIND (`source_editor` degradation), never about a computed
field, so a `Null` flowed through the resolver and was rendered without any
invariant noticing. Nothing compares the two arms' resolvers: no test
asserts that a profile field computed under Turso computes the same under
Loro. Secondary ENVIRONMENT: the two arms assemble their `LiveEntities` at
completely separate seats with no shared definition, so the Loro seat could
be (and was) simply omitted.

## Remedy

FIXED 2026-07-29 — `holon_profiles::LiveEntitySpec` is now the single seat:
`languages()` is the one definition, the Turso SQL predicate and the
in-memory matcher both derive from it, and the Turso DI wiring, the Loro
session and the PBT oracle (which had its own hand-rolled copy) all build
from it. The Loro resolver derives both entities from its `BlockQuerySource`
and refreshes them, gated on `BlockQuerySource::change_version` (Loro doc
lamport height) so an idle doc costs no tree walk. Red-first:
`tests/loro_live_entity_wiring.rs` 3/4 red with `left: Null` (lookup absent,
not a wrong boolean) → 10/10 green with `ref_entity_lookup_parity`.
