---
id: 2026-07-28-keystone-oracle-evaluated-bundled-profile-bare
date: 2026-07-28
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Keystone oracle evaluated the bundled `block` profile on a bare
  `rhai::Engine::new()`, so the profile's `query_source(id)` /
  `rule_sibling(id)` entity lookups (registered in prod by
  `register_entity_lookups` from the ProfileResolver's live entities) were
  absent: `has_query_source` and `is_program` silently evaluated to `Null` on
  the oracle side and one 1-case keystone run logged 6,270 `Function not
  found: query_source` warns
source_line: 790
---

## Bug

Keystone oracle evaluated the bundled `block` profile on a bare
`rhai::Engine::new()`, so the profile's `query_source(id)` /
`rule_sibling(id)` entity lookups (registered in prod by
`register_entity_lookups` from the ProfileResolver's live entities) were
absent: `has_query_source` and `is_program` silently evaluated to `Null` on
the oracle side and one 1-case keystone run logged 6,270 `Function not
found: query_source` warns

## Root cause

the keystone oracle evaluated the bundled `block` profile on a BARE
`rhai::Engine::new()` (`reference_state.rs` `resolve_profile`), so the
profile's entity-lookup calls — `query_source(id)`, `rule_sibling(id)`,
registered in production by `holon_profiles::register_entity_lookups` from
the ProfileResolver's live entities — resolved to nothing. Every
lookup-dependent computed field (`has_query_source`, `is_program`) silently
became `Null` on the oracle side, and one `PROPTEST_CASES=1` keystone run
emitted 6,270 `WARN C4 enrich: computed field eval failed on PRESENT columns
— Function not found: query_source`. Consequences: the oracle could never
predict the `query_block` (`live_block()`) or `rule_card` variants, so no
keystone case could go red on their selection; and the flood buried every
other warn in the keystone log. ENVIRONMENT, not ORACLE: the invariants and
the profile are shared with prod verbatim — only the harness's
engine-construction seat diverged from the production one. Fix:
`holon_profiles::build_lookup_engine` is now the single registration seat
(prod `ProfileResolver` + oracle both build through it), and the oracle
feeds it live entities derived from its OWN block tree — the model's answer
to `query_source_blocks_sql` / `rule_head_blocks_sql`, keyed by `parent_id`,
same projected columns. Red-first proof:
`crates/holon-integration-tests/tests/ref_entity_lookup_parity.rs`, 4/4 red
with `Some(Null)` where `Some(Boolean(_))` is required, 4/4 green after;
`just keystone-smoke` flood 6,270 → 0.)

## Missing piece

Oracle-side Rhai engine had no entity-lookup seat: prod builds its engine
from `LiveEntities`, the harness built a naked engine, so the `query_block`
/ `rule_card` variants were unpredictable by the model and the log was
flooded

## Remedy

FIXED — `holon_profiles::build_lookup_engine` is the shared seat; oracle
derives its live entities from its own block tree
(`ReferenceState::profile_engine`); red-first proof in
`tests/ref_entity_lookup_parity.rs` (4/4 red `Some(Null)` → green),
keystone-smoke flood 6,270 → 0
