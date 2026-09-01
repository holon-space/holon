---
id: 2026-09-01-consolidator-migrate-wipe-unpinned
date: 2026-09-01
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  No test fails when the interim consolidator-handover wipe is skipped, so
  HOLON_CONSOLIDATOR_MIGRATE=1 could silently stop clearing durable state.
---

## Bug
Not an observed defect — an absent oracle, found by inversion while fixing
[[2026-09-01-loro-restart-flip-ignored-invariant-10-epoch]].

Making `wipe_durable_state` (`crates/holon-app/src/consolidator_epoch.rs:155`)
a no-op leaves the whole suite green: 3/3 passes of
`loro_restart_unseeded_vault::restart_with_loro_enabled_over_populated_sql_vault`
(`lane-logs/red-teeth-nowipe.1788253488.log`). The acknowledged flip then boots
over a stale `direct`-epoch Turso database and nothing notices.

## Root cause
`HOLON_CONSOLIDATOR_MIGRATE=1` is the ONLY supported consolidator handover today
(Model.md invariant 10; the state-preserving migration is spec 0008 Phase 4.1,
unbuilt). Its whole job is the wipe — without it the two corruption classes the
invariant names are exactly what you get: bases diffed against another
consolidator's linear history, and `gen_key_between` sort_keys mixed into the
Loro-fi keyspace. The restart test proves the flip is REFUSED unless
acknowledged (inverting the acknowledgement is red 3/3,
`lane-logs/red-teeth-nowipe.1788254428.log`), but it asserts nothing about what
the acknowledgement then does.

## Missing piece
An assertion on the wipe's effect. The unit tests in `consolidator_epoch.rs`
cover the guard's decision, not the durable-state outcome observed through a
real boot; the integration twin observes the boot but not the wipe.

## Remedy
OPEN. Candidate oracle for the restart test's phase 2: assert the durable paths
were actually unlinked across the flip (capture the db inode/mtime before, or
assert no phase-1 `sort_key` in the `gen_key_between` keyspace survives into
the re-seeded Loro-fi rows). Cheap to add; deliberately not bundled into the
`loro-reds` red-fixing lane so its diff stays the two reds plus the gate recipe.
