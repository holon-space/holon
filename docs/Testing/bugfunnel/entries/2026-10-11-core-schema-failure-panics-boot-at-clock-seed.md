---
id: 2026-10-11-core-schema-failure-panics-boot-at-clock-seed
date: 2026-10-11
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A stored block_raw the core schema refuses made CoreSchemaModule skip the clock table, and the boot then panicked where the clock scheduler seeds the clock relation.
---

## Bug
A database holding a `block_raw` that its declaration refuses (a row the
declared shape cannot hold, so the reshape fails) aborted the boot with
`boot [component=turso stage=engine-resolve]: BackendEngine initialization
failed: [DI] clock scheduler failed to seed the clock relation at boot …
no such table: clock`. Found by the verifier of the AUDIT-boot.a lane, outside
any automated test.

## Root cause
Two faults in a row.

- `CoreSchemaModule::ensure_schema` (`crates/holon-turso/src/schema_modules.rs`)
  ran `block_raw` first and stopped at its first error, so `files`, `clock`,
  `clock_reader` and `integration_cache` were never created, although none of
  them depends on `block_raw`. The module failure itself was already disclosed
  as `schema-module-failed`.
- `create_initialized_engine` (`crates/holon/src/di/registration.rs`) returned
  the clock scheduler's seed error with `?`, and the `BackendEngine` DI factory
  turned every error from it into a panic with `.expect`.

Red logs: `target/bcp-red-keep.log` (the block_raw case, the verifier's exact
panic) and `target/bcp-exp2.log` (the clock case: a stored `clock` view, the
same panic with `no such column: epoch_day`).

## Missing piece
No test boots over a database whose core relations an older or foreign binary
left in a shape the declaration refuses. The stale-type-table tests cover
free-standing type tables only, and the keystone always boots a database its
own schema created.

## Remedy
- `CoreSchemaModule` sets up each core relation on its own and reports every
  failed one in one error, so a refused `block_raw` leaves the clock standing.
- A clock scheduler that does not start raises the new `clock-not-running`
  condition and the engine boots without it. `hold_clock_grain` then returns
  an error that names the condition; a query that reads a fine grain is
  already refused.
- `crates/holon-app/tests/core_schema_failure_boots_degraded.rs` boots over a
  refused `block_raw` (rows kept, `schema-module-failed` naming `block_raw`,
  no `clock-not-running`) and over a `clock` view (`schema-module-failed`
  naming `clock` and `clock-not-running`).
