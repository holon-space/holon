---
id: 2026-09-01-subtree-share-tmp-leftover-race
date: 2026-09-01
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  `subtree_share_round_trip_pbt` asserted that no `.loro.tmp` exists in
  `shares/` at an instant of its choosing, but the temp half of an atomic
  publish legally exists while the publish runs.
---

## Class

`FALSE-ALARM`: no product defect existed behind this failure. The publish path
did exactly what it is designed to do, and the test observed it mid-flight. The
oracle was stronger than the property under test — `no tmp exists` rather than
`no tmp remains` — which is the skill's false-alarm definition, so the entry is
excluded from the four-class escape distribution that ranks test investment.

Two real defects were found while root-causing this one, and each has its own
entry rather than riding along here:
`2026-09-02-corrupt-shared-on-a-unsettled-write-oracle` (FALSE-ALARM, FIXED,
but a genuine pre-existing red on `main`) and
`2026-09-02-shared-snapshot-tmp-path-torn-write` (ORACLE, OPEN, product).

## Bug

`sync_pbt::tests::share_subtree_pbt::subtree_share_round_trip_pbt` (target
`sync_suite`) intermittently failed its `P-NO-TMP-LEFTOVER` property. Found by
a hygiene lane measuring the test's isolated rate, not by a suite run.

Decoded payload, `lane-logs/flakes/subtree-1.log:81`, panic at
`crates/holon/tests/sync_suite/sync_pbt.rs:803`:

```
P-NO-TMP-LEFTOVER/B: stale tmp files:
["/var/folders/.../T/.tmpMjfFmI/shares/9cc1d93a-a764-463e-b88d-436458a594f4.loro.tmp"]
```

Rate before the fix, measured 2026-09-01 at base `ed38a4dae833`, 10 isolated
runs: 9 passed, 1 failed. Re-measured 2026-09-02 at base `f27c79b7db4d` under a
heavier load shape (10 rounds of 2 concurrent copies of the `sync_suite` binary,
20 runs): 20 passed, 0 failed. The flake did not fire in that sample, so no rate
measured here separates the two sides — the evidence is the deterministic
reproduction below.

## Root cause

The publish is `create tmp → write → fsync → rename → fsync dir` inside one
synchronous call (`crates/holon-loro/src/shared_snapshot_store.rs:89-133`), so a
`<id>.loro.tmp` exists for exactly as long as that call runs, on the deliberate
design that a torn write leaves the previous snapshot intact. A sweep that lands
in that window sees a file the design says will be gone.

Measured, not inferred. Instrumenting the sweep to re-check after 3 seconds
caught 8 hits, and the file was gone on every one of them; one was caught at
`len=Some(0)`, the instant after `File::create`
(`lane-logs/diag-pbt2.log`, the `DIAG/A` lines).

`Action::SettleSaves` tied its sweep to the writes with nothing but a fixed
400 ms sleep. Two writers can start a publish after that sleep expires:

- Rehydration spawns a detached kick-sync per share that had known peers, with
  three attempts and a backoff starting at one second
  (`crates/holon-loro/src/loro_share_backend.rs`, the `if had_peers` block). Its
  `sync_with_peers` imports remote ops, which commits the shared doc, which arms
  the `any_commit()` save worker (`SAVE_DEBOUNCE` 150 ms). That publish begins
  after the restart action has already returned.
- `SYNC_DEBOUNCE` is 500 ms, longer than the sleep, so the auto-resync worker
  can re-arm a save after it too.

Deterministic reproduction: replacing the sleep with a single worker-quiescence
wait and sweeping immediately turns the flake into a hard failure on the shrunk
input `[RestartA, SettleSaves]`, on 9 of 10 cases
(`lane-logs/flakes/smoke-1-1.log`, `lane-logs/diag-pbt2.log`) — the same
`P-NO-TMP-LEFTOVER` signature as the flake, because the kick-sync's publish
lands just after the settle.

## Missing piece

An oracle that admits the transient, and a settle point to hang it on. The
property read "no tmp exists"; the only form the design promises is "no tmp
remains". Separately, the debounced commit workers exposed no way to ask "is
your window closed and your work call finished?", so the harness had nothing to
wait on and used a sleep.

## Remedy

FIXED.

1. `debounced_commit_worker` tracks work: the Loro subscription counts accepted
   commits, the loop raises a completion counter to the value each work call
   covered, and `DebouncedCommitWorkerHandle::quiesce()` returns a cloneable
   `WorkerQuiesce` with `is_idle()` / `wait_idle()`
   (`crates/holon-loro/src/debounced_commit_worker.rs`). A commit arriving
   during a work call is deliberately not covered by it.
2. `LoroShareBackend::wait_for_workers_idle(scope)` awaits the per-share
   workers to a fixed point. `SettleScope::LocalWrites` covers the save and
   projection workers; the sweep uses it because including the sync worker would
   price the settle in peer reachability rather than in pending local work.
   `SettleScope::IncludingSync` adds the sync worker, for callers that need
   nothing to rewrite the snapshot afterwards.
3. `Action::SettleSaves` settles both peers, sweeps, and retries for up to 30 s
   while any tmp is present, with the budget starting after the first settle so
   the settle cannot consume it. A transient always clears; an orphan never
   does, so a real leak still fails hard.

Three unit tests pin the contract, all in
`crates/holon-loro/src/loro_share_backend.rs`, using a test-only
`set_publish_stall` hook that holds the window open between the tmp write and
the rename:

- `a_fixed_sleep_can_land_inside_the_publish_window` — the mechanism: with a
  stalled publish the tmp is observable and the worker is not idle.
- `worker_quiesce_is_a_publish_settle_point` — after `wait_idle` there is no
  `.tmp` and the snapshot is on disk.
- `settle_stays_bounded_while_the_sync_worker_dials_an_unreachable_peer` — a
  `LocalWrites` settle stays under 3 s while the sync worker holds a barrier
  save open and dials a TEST-NET-1 address, and asserts the worker is still busy
  so the bound is not vacuous.
- `settling_including_sync_keeps_a_corrupt_snapshot_corrupt` — an
  `IncludingSync` settle leaves no writer that could republish over bytes
  written after it.

Red-for-the-right-reason logs, both in the `subtree-share-race` lane:

- `lane-logs/red-settle-point.log` — the settle-point test with the settle
  reverted to the pre-fix 400 ms sleep: `quiesced save worker left a .tmp behind
  ... shares/stalled.loro.tmp`.
- `lane-logs/RED-settle-budget.log` — the unreachable-peer test before the sync
  worker was excluded: `settle took 35.544849125s; it waited on the sync
  worker's barrier save and its dial to an unreachable peer`.
- `lane-logs/RED-corrupt-settle.log` — the corruption test against a local-only
  settle: `a barrier save republished over the corruption after the settle
  returned`.

The test is removed from the land gate's sanctioned flake list in
`DEVELOPMENT.md` and `docs/Testing/KeystoneKnownReds.md`, and remains
deliberately unregistered as a known-red.

## Keystone repro

Not attempted. `general_e2e_composed_pbt` has no share/publish transition in its
catalog, so it could not draw this interaction as things stand.
