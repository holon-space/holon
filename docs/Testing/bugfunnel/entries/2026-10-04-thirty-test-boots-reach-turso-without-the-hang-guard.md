---
id: 2026-10-04-thirty-test-boots-reach-turso-without-the-hang-guard
date: 2026-10-04
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Thirty test boots that reach a Turso engine arm no database-stuck guard, and nothing detects a missing arm.
---

## Bug
Found by the verifier of hang-detector round 9 (verify-hang-5, code audit), claim 4. All 17 `new_from_config_with_di` boots in `crates/holon-app/tests` are armed.

## Root cause
Twenty-eight holon-app test binaries boot a Turso engine by another path (`create_test_engine_with_providers`, a direct `TursoBackend`, hand-built fluxdi modules) and do not reference `database_stuck_guard`; the list is in `verify-hang-5/verify.md` claim 4. Two harness boots pass `|_| Ok(())` and arm nothing: `crates/holon-integration-tests/src/test_environment.rs:676` (`SecondWriterRefused`) and `:768` (`EpochFlipRejected`). `assert_guarded_in` is called only from `test_environment.rs:519`, `:1112` and `frontend_slice/components.rs:976`.

## Missing piece
An arm at every boot path that reaches an engine, and a check that fails a boot test that lacks one.

## Remedy
Open. Not fixed in round 10. Arm the paths, or move the arm into the one function every engine boot goes through, so a new boot cannot forget it.
