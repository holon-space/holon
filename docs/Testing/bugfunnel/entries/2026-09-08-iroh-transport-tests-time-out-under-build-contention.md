---
id: 2026-09-08-iroh-transport-tests-time-out-under-build-contention
date: 2026-09-08
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  iroh-backed share tests fail with "connection lost" / PATH_ABANDON timeouts
  whenever the machine is saturated by builds, and pass 3 of 3 in isolation —
  the wall-clock transport timeouts are shorter than scheduler latency.
---

## Bug

Found by the `w-sql-loss` lane's full nextest run on a machine busy with cold
builds. Two `holon-loro` transport tests fail at
`scratchpad/w-sql-loss-nextest.48157.log` lines ~1862-1882:

    thread 'loro_share_backend::tests::accept_orphan_target_lands_under_shared_with_me_root' (2303378) panicked at crates/holon-loro/src/loro_share_backend.rs:4935:14:
    called `Result::unwrap()` on an `Err` value: "initial sync failed: [init] drain recv stream: drain read failed: connection lost"

    thread 'loro_share_backend::tests::accept_page_adopts_identity_and_folds_root' (2303386) panicked at crates/holon-loro/src/loro_share_backend.rs:4768:14:
    called `Result::unwrap()` on an `Err` value: "initial sync failed: [init] Failed to read peer delta: Failed to read frame length: connection lost: detected an error with protocol compliance that was not covered by more specific error codes: peer failed to respond with PATH_ABANDON in time"

Replayed in isolation on a quiet machine, the same two tests pass 3 of 3
(`scratchpad/rerun-share-backend-{1,2,3}.log`, each
`Summary [ 3.87s] 2 tests run: 2 passed, 360 skipped`).

## Root cause

Not the code under test. Three independent lanes see the same shape and none of
them touches transport:

- This lane's failures disappear in isolation, 3/3 green in under 4 seconds each.
- The D94 lane
  (`.claude/worktrees/pair-boot-degraded/lane-report-pair-boot-degraded.md:113`)
  reports the same suite failing on four members in parallel, and on a serial
  `-j1` re-run (`Summary [177.406s] 48 tests run: 47 passed, 1 failed, 317
  skipped`) all four pass while a *different* member,
  `accept_page_adopts_identity_and_folds_root`, fails instead. A failure that
  moves between members of a suite under a scheduling change is timing, not
  behaviour.
- The D93 lane sees `gated_share_rejects_forged_ticket_serves_enrolled_peer`
  fail with the same connection-lost signature.

The QUIC error text names the mechanism directly: the peer "failed to respond
with PATH_ABANDON in time". Both endpoints live in the same test process, so
when the machine is oversubscribed the responding task is simply not scheduled
inside the protocol's wall-clock deadline, and the transport tears the
connection down as if the peer were gone.

## Missing piece

The iroh transport tests use wall-clock timeouts sized for an idle machine, with
no relation to how much CPU the test process is actually being given. Under a
saturated scheduler the deadline expires before the in-process peer runs, so the
suite reports a transport failure for a condition that has nothing to do with
the transport — and, as the D94 lane shows, it reports it against a different
test each run, which makes the signal useless for attribution.

## Remedy

Open. Either scale these tests' transport deadlines by observed scheduler
latency, or run the iroh suites under a serialisation token so they never
compete with a cold build. Until then a connection-lost failure in
`loro_share_backend` or `iroh_advertiser` must be confirmed by an isolated
replay before it is attributed to a lane's diff.
