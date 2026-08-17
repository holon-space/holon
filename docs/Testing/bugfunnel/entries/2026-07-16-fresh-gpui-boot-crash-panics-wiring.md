---
id: 2026-07-16-fresh-gpui-boot-crash-panics-wiring
date: 2026-07-16
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  P1 fresh-DB GPUI boot CRASH: `seed_default_layout` panics `no such table:
  block_history` (wiring.rs:352) — `BackendEngine::new` wires
  `TursoHistoryStore` (C2b) over the raw handle but NOTHING in the lazy-DI
  gpui graph resolves `DbReady<HistoryTables>` (zero consumers repo-wide);
  `with_dependency` is a graph-tooling hint with no runtime effect, so the
  engine provider's declared dep list was cosmetic
source_line: 817
---

## Bug

P1 fresh-DB GPUI boot CRASH: `seed_default_layout` panics `no such table:
block_history` (wiring.rs:352) — `BackendEngine::new` wires
`TursoHistoryStore` (C2b) over the raw handle but NOTHING in the lazy-DI
gpui graph resolves `DbReady<HistoryTables>` (zero consumers repo-wide);
`with_dependency` is a graph-tooling hint with no runtime effect, so the
engine provider's declared dep list was cosmetic

## Missing piece

keystone/composed wirings eager-resolve schema markers; no test boots the
gpui lazy-DI path on a fresh DB

## Remedy

FIXED this session (dogfood #6): engine factory now
`resolve_async::<DbReady<HistoryTables>>()` before construction
(crates/holon/src/di/registration.rs); verified clean boot. Gap CLOSED
2026-07-17:
`crates/holon-app/tests/fresh_db_boot_seed_smoke.rs::fresh_db_boot_reaches_seed_without_history_tables_panic`
boots the lazy-DI engine factory against a fresh FILE-backed Turso DB and
asserts `seed_default_layout` completes + `block:root-layout` lands (proving
the history-recording creates ran). Negative-check finding: the
`registration.rs` `DbReady<HistoryTables>` resolve is now REDUNDANT —
`all_schema_roots()` also materializes `block_history` via
`AutomationsJournalView`, so reverting the registration.rs lines alone no
longer reproduces the panic (the test only goes red when BOTH boot-time
creators are removed; verified). The explicit resolve is dead
belt-and-suspenders; removing it is a separate call.
