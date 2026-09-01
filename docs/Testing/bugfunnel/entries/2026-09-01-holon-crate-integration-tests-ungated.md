---
id: 2026-09-01-holon-crate-integration-tests-ungated
date: 2026-09-01
gap: ENVIRONMENT
secondary: ORACLE
status: PARTIAL
summary: >-
  No gate runs the `holon` crate's integration tests, so 26 failures accumulated
  unseen — including a p2p sync adapter that had been broken for every peer.
---

## Bug

`cargo nextest run --no-fail-fast -p holon-kitchen -p holon-core -p holon
-p holon-app` on main (`ed38a4dae833`) reports **25 failed + 1 timed out** of
885, entirely in the `holon` crate's integration tests
(`lane-logs/ab-holon-main.nextest.log`, `Summary [ 227.905s] 885 tests run:
859 passed (8 slow, 1 leaky), 25 failed, 1 timed out, 5 skipped`). The landing
gate runs `holon-app` only, so none of these tests is executed by any gate and
the reds accumulated across many landings.

Found by orchestrator census, triaged in lane `reds-triage`.

## Root cause

Two independent product/test defects were hiding behind the missing gate:

1. **`IrohSyncAdapter::new` binds an endpoint with zero ALPNs**
   (`crates/holon-loro/src/iroh_sync_adapter.rs:462`,
   `Endpoint::builder().bind()`). Under iroh 0.96 an endpoint that advertises
   no ALPN rejects every peer at the handshake — `error 120: peer doesn't
   support any known protocol`. The sibling constructor that registers ALPNs,
   `new_with_alpns`, has **zero callers anywhere in the workspace**. 14 sync
   tests failed on this one cause.

2. **`test_parallel_sync_operations` never listened on the address it handed
   out** (`crates/holon/tests/stress_tests.rs:193-203`): it published
   `hub_adapter.addr()` but spawned its five accept loops on five *different*
   adapters, each with its own endpoint. The clients dialled an address nobody
   accepted on, so the test hung to the 120s nextest timeout.

The ORACLE secondary: `test_alpn_mismatch_detection` **passed vacuously** the
whole time. It asserts only that accept returns an error, and it got one for
the wrong reason — no ALPN was ever registered, so the matching case failed
identically to the mismatching one. Likewise
`reliability_tests::test_sync_with_empty_peer` asserted
`assert_eq!(text2, "")` — i.e. that sync transferred *nothing* — which was
only true while sync was broken.

## Missing piece

No gate executes `-p holon`'s integration tests. Note the crate cannot be
tested alone: `-p holon` by itself does not compile them, because the
`test-helpers` feature is only unified when `holon-app` is in the same
invocation.

## Remedy

Fixed in this lane: ALPN registration at the accept seam
(`accept_sync` now calls `endpoint.set_alpns`), the parallel-sync accept
topology, and the two inverted oracles. 26 failures → 7
(`lane-logs/gate-full.nextest.txt`, `Summary [ 248.400s] 884 tests run: 877
passed (6 slow), 7 failed, 5 skipped`).

Still OPEN: the gating decision itself (add `-p holon` to the D43.a parallel
nextest, ~250s wall clock, vs. a nightly tier) — see
`docs/Testing/HolonCrateReds-2026-09-01.md`.
