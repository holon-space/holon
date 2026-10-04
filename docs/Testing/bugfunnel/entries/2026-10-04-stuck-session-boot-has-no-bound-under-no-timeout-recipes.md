---
id: 2026-10-04-stuck-session-boot-has-no-bound-under-no-timeout-recipes
date: 2026-10-04
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  A stuck SQL command during a keystone session boot hangs a no-timeout `cargo test` recipe until the guard ends it, 49 min after the command started.
---

## Bug
Found by the verifier of hang-detector round 9 (verify-hang-5, code audit). The hang detector lane (D39.b) reports a stuck command from 30 s on, but in `just pbt *` and `just hand-authored` (step 13/17 of `just land`) nothing ends a stuck boot before the guard.

## Root cause
`init_test` boots the SUT with `rt.block_on(S::build(..))` and `feed_sut_clock` with no timeout (`crates/holon-integration-tests/src/pbt/composed/harness.rs:1370`). The keystone wedge is a `tokio::time::timeout` only around apply+settle (`harness.rs:483`, `:1399`) and `Reboot` (`:1099`). `within_cycle_watchdog` is armed at two sites only (`frontend_slice/components.rs:8007`, `:8084`). The recipes run `cargo test` with no timeout (`justfile:154`-`:177`, `:275`, `:299`, `:488`, `:548`, `:654`, `:767`, `:904`, `:1424`, `:1510`). Source: `lane-logs/hang-detector/round-9` and `verify-hang-5/verify.md` defect 2b.

## Missing piece
A bound on the session boot in the harness, equal to the wedge bound on a transition.

## Remedy
Open. Not fixed in round 10. Bound `S::build` and `feed_sut_clock` in `init_test` with the same wedge as a transition, or arm `within_cycle_watchdog` around the boot.
