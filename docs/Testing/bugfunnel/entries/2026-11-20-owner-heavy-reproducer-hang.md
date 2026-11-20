---
id: 2026-11-20-owner-heavy-reproducer-hang
date: 2026-11-20
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  `owner_heavy_indent_then_join_stalls_the_receiver_projection` reds 1 run in 13
  on main (the receiver keeps a block the owner joined away), and a plain `cargo test` run of it
  has no time cap, so a hang there holds a slot for hours.
---

## Bug

A verifier ran the fixed-script test `two_instance_composed_pbt::owner_heavy_indent_then_join_stalls_the_receiver_projection`
ten times in a loop with `cargo test ... --exact` instead of nextest. The first run hung with no result for more than four hours.
A second red was seen in a lane nextest run. Found by a verifier, not by an automated gate.

## Root cause

Two separate findings, measured on the `test` build profile (unoptimized + debuginfo), `HOLON_PBT_BUDGET=lane`, one run at a time.

- The hang ran outside nextest. `.config/nextest.toml:36-38` caps every test of the binary `holon-integration-tests::integration`
  whose name starts with `two_instance_composed_pbt::` at 20 minutes. `cargo nextest list` with that exact filter lists the
  owner-heavy test, so the cap applies to it under nextest. `cargo test` has no cap. No justfile recipe runs this test through `cargo test`.
- The test itself reds intermittently. In 13 nextest runs on main it reds once, at `two_instance_composed_pbt.rs:811:5`
  ("the two peers did NOT converge after 5 write(s) and a sync fixed point. 2 block(s) differ"). The owner holds
  `block:archive-100` with the joined content and no `block:archive-101`. The receiver still holds `block:archive-101`
  under `block:archive` and `block:archive-100` without the joined text. The failed run took 7.5 s, so it is not a timeout.
  The shape matches the join not reaching the receiver's projection. Whether it is the cause class of
  `2026-09-02-receiver-projection-stalls-after-one-failed-reconcile` is not established: the failed run's log holds no reconcile error line.
- On base 9ce0dbde the test is green in 10 of 10 runs that started the test. An 11th run was killed by the outer 25 minute limit during
  compilation of a fresh tree, before the test started.
- Host load was between 30 and 72 for all runs; load below 10 was not reached in 2 hours of waiting.

## Missing piece

The red has no known cause and no registry row. The verifier's loop had no time cap. A fixed-script test with 1 red in 13 runs
cannot be told from a load effect with the 13 runs available, and a load-driven stall reads as non-convergence.

Why ENVIRONMENT (litmus):
- The hang: the gate runs the test under nextest with a 20 minute cap; the verifier ran it under a bare `cargo test` with none. The test and the code are the same; only the run environment differs.
- The red: it appeared inside an automated nextest run, not in prod, and only under host load 30 to 72. Whether it is a logic defect or a load effect cannot be told until it reproduces at low load; until then it stays with the environment finding, OPEN.

Parity work needed: every bounded run of this test goes through nextest (or any bare-cargo recipe gets the same outer cap), and the rate must be measured once at load below 10.

## Remedy

Open. Fix card or known-reds row is for main to rule. The cap in `.config/nextest.toml` needs no change. Run this test through nextest, never `cargo test`, when it must be bounded.
