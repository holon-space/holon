---
id: 2026-09-02-corrupt-shared-on-a-unsettled-write-oracle
date: 2026-09-02
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  `subtree_share_round_trip_pbt`'s `CorruptSharedOnA` wrote its corrupt bytes
  without settling, and the reference assumed a corruption survives later
  legitimate writes, so `P-REG/A` went red whenever a save republished over it.
---

## Class

`FALSE-ALARM`: no product defect existed. `P-REG/A` fired, and the product was
right on both counts — a debounced save republishes the snapshot, and so does
any later commit on A. The reference model assumed more than the product ever
promised, which is an oracle stronger than the property under test rather than
a missing invariant letting a real defect through. The entry is therefore
excluded from the four-class escape distribution.

The signature is a genuine pre-existing red on `main` (2 of 6 base-side runs,
below), so it was worth fixing; it just is not an escape.

## Bug

`sync_pbt::tests::share_subtree_pbt::subtree_share_round_trip_pbt` fails
`P-REG/A: manager registration diverged from ref` at
`crates/holon/tests/sync_suite/sync_pbt.rs:577` — the SUT still has A's shared
doc registered after a corrupt-then-restart while the reference expects it gone.

Found while root-causing
`2026-09-01-subtree-share-tmp-leftover-race`: removing that test's 400 ms sleeps
stopped hiding this one. It is a distinct defect with a distinct signature, so
it gets its own entry.

Measured on the UNMODIFIED base binary at `f27c79b7db4d` by a fresh-context
verifier: **2 failures in 6 runs**, both this signature, both shrunk to
`[CrossPeerSyncAfterRestart(" [X:a]"), CorruptSharedOnA, RestartA]`
(`lane-logs/subtree-share-race-verify.md` §8). It is therefore a real pre-existing
red, not something the lane introduced.

Two more shrunk inputs from the lane's own runs, on builds that had the sleeps
removed but this defect unfixed:

- `[EditOnA, CorruptSharedOnA, RestartB, RestartA]` — `lane-logs/flakes/after-3-1.log`
- `[EditOnA, CorruptSharedOnA, MarkOnA, RestartA]` — `lane-logs/flakes/after2-2-1.log`

## Root cause

Two independent mistakes, both in the harness, both about the same product
behaviour: a commit on A's shared doc republishes the snapshot.

1. **The corruption was written without settling.** `CorruptSharedOnA` truncates
   `<id>.loro` to a few bytes. A preceding edit arms the debounced save worker
   (`SAVE_DEBOUNCE` 150 ms), and if that save fires after the bytes land it
   rewrites a valid snapshot over them. Rehydration then succeeds and A stays
   registered.
2. **The reference assumed corruption is durable.** Even with the write settled,
   any later commit on A republishes the snapshot, and `RestartA` calls
   `flush_all` whenever `corrupt_pending` is false, so the file is valid at
   rehydration. The model kept `corrupt_pending` set across `EditOnA`,
   `MarkOnA` and `PullBtoA` and predicted an unregistered share.

`CrossPeerSyncAfterRestart` already skipped itself when `corrupt_pending` was
set, with a comment naming this exact ambiguity. The other actions were never
given the same treatment.

## Missing piece

The reference model had no rule for "a write repairs a pending corruption",
even though the production path plainly performs one. The invariant existed and
was correct; the state it was compared against was wrong.

## Remedy

FIXED, in `crates/holon/tests/sync_suite/sync_pbt.rs`:

- `CorruptSharedOnA` awaits
  `LoroShareBackend::wait_for_workers_idle(SettleScope::IncludingSync)` on A
  before writing its bytes, so no already-armed save can republish over them.
  The scope matters: `sync_with_peers` republishes the snapshot as its
  save-before-push barrier *before* it dials, so the sync worker is one of the
  writers that must be settled here. The sweep in `SettleSaves` uses
  `SettleScope::LocalWrites` instead, because there the sync worker's dial cost
  would price the settle in peer reachability.
- `EditOnA`, `MarkOnA` and `PullBtoA` clear `ref_a.corrupt_pending`, matching
  what the product does.

Re-measured after the fix, same load shape as the base measurement (the
prebuilt `sync_suite` binary, 2 concurrent copies per round, whole loop under
`sem --id holon-build -j4`): the verifier saw **0 of 22** after-side runs hit
this signature, against 2 of 6 on the base binary.

Pinned by `settling_including_sync_keeps_a_corrupt_snapshot_corrupt` in
`crates/holon-loro/src/loro_share_backend.rs`, which settles, writes a 10-byte
corrupt payload, waits three sync-debounce windows and asserts the payload is
still there. Red against the local-only scope in
`lane-logs/RED-corrupt-settle.log`: `a barrier save republished over the
corruption after the settle returned`, with a full LORO snapshot on disk in
place of the 10 bytes.

Residual, not fixed: if peer B holds ops A lacks, a restart of B can push them
into A and commit A's shared doc, repairing the corruption with no A-side action
to clear `corrupt_pending`. Closing that needs the reference to model cross-peer
divergence. Not observed in any run on this tree.

## Keystone repro

Not attempted. `general_e2e_composed_pbt` has no share, corrupt or restart
transition in its catalog, so it cannot draw this sequence.
