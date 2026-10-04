---
id: 2026-10-04-wedge-cannot-fire-on-a-current-thread-composed-slice
date: 2026-10-04
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  On a composed slice that runs on a current_thread runtime, the keystone wedge cannot end a stuck SQL command, because the spawned actor holds the only worker.
---

## Bug
Found by the verifier of hang-detector round 9 (verify-hang-5, code audit), defect 2c.

## Root cause
`ComposedSlice::MULTI_THREAD` defaults to `false` (`crates/holon-integration-tests/src/pbt/composed/harness.rs:221`; runtime built at `:1359-1362`). `FrontendStructural` (`frontend_slice/structural_pbt.rs:299`) does not set it. The Turso actor is `tokio::spawn`ed onto that runtime (`crates/holon-turso/src/turso.rs:2177`), and a stuck command never yields (`actor_watch.rs:6-9`), so `tokio::time::timeout` never fires. Under nextest a 600 s override ends the run; under `cargo test` only the guard does.

## Missing piece
A multi-thread runtime for every slice that boots a real Turso actor, or a wedge that runs on an OS thread.

## Remedy
Open. Not fixed in round 10.
