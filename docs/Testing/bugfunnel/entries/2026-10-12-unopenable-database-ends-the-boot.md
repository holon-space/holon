---
id: 2026-10-12-unopenable-database-ends-the-boot
date: 2026-10-12
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A database that the open cannot use (held lock, I/O refusal, failed move) ends the boot,
  where the boot-always directive asks for a degraded no-Turso boot with a banner.
---

## Bug
Found by the honeymoon2 boot-ladder census (code audit, lane boot-ladder-gaps).

## Root cause
`crates/holon/src/di/lifecycle.rs` `open_and_register_core` returns the open error with `?`.
GPUI `main` exits on `BootError`. No no-Turso session exists in prod: the Loro-only session
assembly lives only in the test env
(`crates/holon-integration-tests/src/test_environment.rs` `start_app_loro_memory`).

## Missing piece
BootLadder rung 1a (`docs/Plans/BootLadder-2026-07-18.md`, Increment 4): a BootSupervisor
above DI that builds a fresh container without Turso and starts a Loro-only session.

## Remedy
Open, escalated. Design questions no ruling covers:
1. In SqlOnly mode a Loro-only fallback is a consolidator change, which invariant 10 refuses.
   Does rung 1a in SqlOnly mode boot read-only, or stop?
2. What does GPUI leave out without Turso (DebugServices / MCP, ReactiveEngine)?
3. Does `start_app_loro_memory` move into holon-app as the prod assembly?
The open is ready for it: a held lock ends in a typed `Locked` error after a bounded wait
(`2026-10-12-db-open-locked-fails-boot-at-once`), and `OpenFailed` carries every move made
before the failure (`2026-10-12-db-open-failure-drops-earlier-moves`).
