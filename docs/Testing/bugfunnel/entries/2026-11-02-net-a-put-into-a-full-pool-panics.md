---
id: 2026-11-02-net-a-put-into-a-full-pool-panics
date: 2026-11-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  In holon-net, a put into a pool place that already holds its size panicked, from an outside write, a failure or timeout port, an effect result and a verified resolution.
---

## Bug
The tester of card #867 found it (its questions Q4, Q6, Q7) and left an ignored red case; card #882 took it.

## Root cause
`Tokens::put` asserted that the pool had a free slot (crates/holon-net/src/exec.rs:910 on the base f77be010c8f1, "a put into the pool P of N tokens, none of which is taken"). Every put path reaches it: `write`, `run_failed` (on_failure port), the run timer (on_timeout port), `complete(Done)` and `resolve(Verify)`. Red on the base: `a_write_into_a_full_pool_is_refused_and_changes_nothing`, `a_failure_port_into_a_pool_never_overfills_it`, `a_timeout_port_into_a_full_pool_never_overfills_it`, `an_effect_result_into_a_full_pool_never_overfills_it`, `a_verified_resolution_into_a_full_pool_never_overfills_it` (crates/holon-net/tests/caller_input_refusals.rs).

## Missing piece
No generator drew a put into a full pool. Litmus: COVERAGE, because no case reached the state; not ORACLE, because the no-panic oracle of the caller-input machine would have gone red had a case reached it.

## Remedy
`Tokens::put` returns `Err(PoolFull)` before it touches the slot, once for every caller. An outside write returns `ExecError::PoolFull` and changes nothing. A firing's overflowing pool token is not put, its other outputs commit, and `Condition::PoolFull` is disclosed once per place and transition. The firing-side form is provisional until ruling D882.1.
