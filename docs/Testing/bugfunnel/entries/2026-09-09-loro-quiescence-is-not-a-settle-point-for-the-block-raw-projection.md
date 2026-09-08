---
id: 2026-09-09-loro-quiescence-is-not-a-settle-point-for-the-block-raw-projection
date: 2026-09-09
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Under machine contention `prod_session_create_block_persists_to_block_raw`
  reaches Loro quiescence and still finds no `block_raw` row for the block
  `block.create` just reported success for. Loro quiescence does not gate the
  SQL projection, so the test's settle point can pass while the projection is
  still outstanding — leaving it unresolved whether the product really reports
  success before persist, or only the settle point is wrong.
---

## Bug

Seen once in three `just loro-suite` runs on the `share-lifecycle` lane
(2026-09-09), under a concurrent build slot:

```
loro_create_persists_prod_session::prod_session_create_block_persists_to_block_raw
crates/holon-integration-tests/tests/loro_suite/loro_create_persists_prod_session.rs:129
assertion `left == right` failed: under an existing block: block.create returned
success but NO row reached block_raw within the projection budget —
success-before-persist (fail-loud violation)
  left: 0
 right: 1
```

`scripts/keystone-known-reds.sh` classifies the signature **novel** — it is not
one of the pass-with-note gate reds.

Not caused by the lane that found it: rev 5's entire diff is inside
`#[cfg(test)] mod tests` of `crates/holon-loro/src/loro_share_backend.rs` (hunks
at 2918/4555/4716/4737/4852, all after the `mod tests` at :2865) plus the
architecture allow-list. The failing test links holon-loro's **lib**, which rev 5
does not touch. Two immediate re-runs were 16/16 green.

## Root cause

Not established — this entry records evidence, not a post-mortem.

What the evidence rules OUT: a swallowed settle timeout.
`TestEnvironment::wait_for_loro_quiescence`
(`crates/holon-integration-tests/src/test_environment.rs:1408-1417`) ends in
`.expect("wait_for_loro_quiescence: loro sync did not quiesce before the
deadline")`, so a missed 10 s deadline panics with that message instead. The
observed panic is the row assertion, which means **quiescence was reached and the
`block_raw` row was still absent**.

So the test's settle point is Loro sync quiescence, while the value it asserts on
is a SQL projection of that Loro commit. Nothing establishes that the second is
complete when the first is. Two hypotheses, undecided:

1. **Settle-point gap (test).** Quiescence gates the Loro sync loop but not the
   reconcile/projection step that writes `block_raw`, so under contention the
   two separate and the assertion samples too early. Then the product is correct
   and the oracle is.
2. **Real success-before-persist (product).** `block.create` returns `Ok` before
   the row is durable, which is exactly the fail-loud violation the assertion is
   written to catch, and contention only widens the window that makes it
   visible.

Distinguishing them means re-sampling `block_rows` after quiescence with a
bounded retry: if the row appears, it is (1); if it never appears, it is (2) and
is a real P1.

## Missing piece

**ORACLE (primary).** The test pairs an assertion about the SQL projection with a
settle point that only covers Loro sync. Whichever hypothesis holds, no invariant
currently states "the block_raw projection has caught up", so the suite cannot
tell a slow projection from a lost one — the failure text asserts
success-before-persist, but the evidence does not support that reading over the
settle-point one.

**ENVIRONMENT (secondary).** Only reproduces under machine contention (a
concurrent cargo build holding the build slot), so an unloaded CI or dev run
will not see it — 1 in 3 here, 0 in 2 on the immediate re-runs.

## Keystone repro

Not attempted. This IS an integration-suite test; the question is its settle
point, not whether the composed keystone can reach the state.

## Remedy

OPEN. Deliberately not fixed in the `share-lifecycle` lane, whose rev 5 scope was
the `LoroDocument::doc()` escape allow-list; the lane neither caused this nor can
adjudicate it without the re-sampling experiment above.

Note for whoever takes it: do NOT simply widen the 10 s budget. The budget is
already generous and quiescence already returned inside it, so a longer wait
tests nothing — the settle point is the suspect, not its length.
