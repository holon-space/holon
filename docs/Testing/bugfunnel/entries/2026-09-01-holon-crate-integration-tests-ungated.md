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

## Dogfood/gate re-run 2026-09-08

`test_turso_backend_state_machine` is live and passing, not the dead red the
census recorded. The version API it was said to drive is absent from the tree
*and* unnamed by the test, so that attribution was never available. Isolated at
`34fca6bf` (`lane-logs/run-sm.log`):

```
Summary [   4.687s] 2 tests run: 2 passed, 0 skipped
```

Both tests in the `turso_storage_pbt` binary pass — the state machine and
`test_view_change_stream_receives_events_from_backend_operations`. Row 20 of
`docs/Testing/HolonCrateReds-2026-09-01.md` and the binary's nextest cap are
corrected accordingly. The gating decision above stays OPEN; a failure of this
test inside a loaded run is unexplained by the version API. A contended run has
since been captured — see below.

## Captured contended run 2026-09-15

Inside a 2149-test nextest run on integration `30e2bc43` (the `gate-entity-uri`
leg), the test failed on the deleted-row event of the transition sequence
`… Update("xui") … Insert("zzhzx", parent "xui") … Delete("xui")`. The change
feed carried the ENTITY ID where the test's rowid→entity map expected the rowid:

```
    === View Change Entity ID Mismatch at index 4 for 'entity_view' ===
    Expected entity ID: Some("xui")
    Actual entity ID: None
    Expected change: Deleted { id: "2", origin: Remote { operation_id: None, trace_id: None } }
    Actual change: Deleted { id: "xui", origin: Remote { operation_id: None, trace_id: None } }
```

`crates/holon/tests/turso_storage_pbt/pbt_tests.rs:1988`. The run closed
`Summary [ 165.662s] 2149 tests run: 2142 passed (7 slow, 1 leaky), 7 failed, 10
skipped`.

Isolated on the SAME tree it is green: the whole `turso_storage_pbt` binary 3/3
(`turso3x-{1,2,3}.log` under the lane scratchpad below, each `Summary [
0.509s] 2 tests run: 2 passed, 0 skipped`) and this test alone twice over 512
cases (`turso512-{1,2}.log`, each `Summary [   0.732s] 1 test run: 1 passed, 1
skipped`). It did not recur in the next 2149-test run of the same gate
(`Summary [ 167.160s] 2149 tests run: 2143 passed (3 slow), 6 failed, 10
skipped`, this test absent from the failing list).

Lane scratchpad holding those logs, the excerpt and the gate log:
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/`.
A 522-line excerpt of the failing run is preserved at
`.claude/worktrees/docs-w14/lane-logs/turso-state-machine-contended-1789429950.log`
— a LANE workspace path, not a tracked one, so it survives only as long as that
workspace does.

The cause is still UNATTRIBUTED — the version API explanation is absent from the
tree, and the trigger is contention-shaped, so nothing here names it. It is a
prod-bug candidate of the CDC kind: a consumer that keys change events by rowid
is handed the entity id instead and resolves `None`, i.e. it sees a deletion it
cannot match to a row it holds. Attribution needs a dedicated triage lane; the
gating decision above is unaffected.
