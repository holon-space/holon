---
id: 2026-11-03-shell-wait-error-orphans-group
date: 2026-11-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net-shell, a run whose wait on the child failed returned the error with the process group still running and still listed as in flight.
---

## Bug
Found by a code audit of the shell effect's run path (card #872); the tester's PBT went red on the base.

## Root cause
`Runner::run` returned the wait error with `?` straight out of its select, which skipped the kill and the pid-file cleanup that follow it.

## Missing piece
No test made the wait on a run's child fail.

## Remedy
The same drop guard as for a dropped run (crates/holon-net-shell/src/effect.rs `Unended`) is still armed when the error returns, so it kills the group until gone and removes the run from `in_flight` before `run` returns the error. Red on the base: /Users/martin/.cache/armc/c872-r2/logs/red-base.log (`a_run_whose_wait_fails_returns_the_error_with_its_group_dead`).
