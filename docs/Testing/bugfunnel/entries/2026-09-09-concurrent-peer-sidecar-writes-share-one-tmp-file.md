---
id: 2026-09-09-concurrent-peer-sidecar-writes-share-one-tmp-file
date: 2026-09-09
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Every atomic write in the shared-snapshot store derived its tmp file name from
  the share id alone, so two concurrent writers of the same sidecar shared one
  file: the faster one renamed the slower one's tmp away. On the revocation leg
  that surfaced as a revocation reporting failure, and in the narrow window
  where both had written over each other's bytes it was a revoked peer's dial
  addr restored on disk while both writers reported success.
---

## Bug

Found by the adversarial verifier pass on the `share-lifecycle` lane (rev 2),
2026-09-09 — outside any test, as a load-dependent red in one of four full-suite
runs, filed there as defect R2-D1. It reproduced 0/6 times in isolation, so it
was contention-dependent, not a deterministic failure of the test.

`revoking_a_peer_stops_every_further_import_from_it` failed at the revocation
call with:

```
the peers sidecar for share e7d7c8bb-… could not be rewritten after revoking
peer PeerFingerprint(f7f5534a…), so that peer's dial addr survives on disk and
comes back at the next restart: rename …/shares/<id>.peers.json.tmp →
…/shares/<id>.peers.json: No such file or directory (os error 2)
```

In production that is a revocation the caller must treat as not having stuck:
the in-memory drop dies with the process, so the next launch reloads the sidecar
and `sync_with_peers` dials the revoked peer again.

## Root cause

Not the shares directory disappearing, which is what the ENOENT reads like at
first — `save_peers` calls `create_dir_all` and then successfully creates a file
in that directory, and nothing in the tree removes it (`pairing_swap.rs:141`
removes a pairing staging dir, never `shares/`).

The tmp path was a pure function of the share id
(`crates/holon-loro/src/shared_snapshot_store.rs`, pre-fix `peers_tmp_path`:
`<shares_dir>/<id>.peers.json.tmp`), and the sidecar has two concurrent
production writers for the same id:

- `LoroShareBackend::remember_peer` (`loro_share_backend.rs:1042` pre-fix),
  reached from `peer_connected_callback`, which runs it in a `tokio::spawn`ed
  task on every admitted inbound dial;
- `LoroShareBackend::forget_peer_addrs` (`:1078` pre-fix), the revocation leg.

Interleaved they share one file: both `File::create` it (the second truncating
the first's bytes), the first to `rename` moves it into place, and the second's
`rename` finds nothing at the source — ENOENT. That is the observed red.

Two consequences beyond the visible one, from the same root: whichever writer
renames last publishes a file BOTH of them wrote into, so the published sidecar
can be a torn mix of two JSON arrays; and each of the two also read
`known_peers` and released the guard BEFORE writing, so an admission that
snapshotted the map before the revocation removed the peer could publish that
stale set last and restore the revoked addr — with both operations returning
`Ok`. The fixed tmp name was accidentally masking that second failure by
destroying the loser's file, which is why fixing only the tmp name turns the
silent variant on (proven: `lane-logs/r3-red-2-fixA-only.log`).

## Missing piece

**ORACLE (primary).** No test asserted anything about two writers of the same
sidecar. The store's tests wrote one file at a time
(`save_and_load_round_trip`, `save_does_not_leave_tmp_file`), and the
share-level tests drove one revocation against a quiescent peer set. The
property "a concurrent save for the same share must not make this writer's
publish fail, nor restore what it removed" did not exist anywhere, so a run that
reached the state still passed except by the accident of a rename error.

**COVERAGE (secondary).** The store had no seam to order two publishes against
each other. `publish_stall` existed but applied to every snapshot publish, so it
could not single out one of two racing writers; the interleaving was reachable
only by luck under machine load, which is exactly how it was found.

## Keystone repro

The keystone (`tests/general_e2e_composed_pbt.rs`) boots one instance and never
shares a subtree, so it cannot reach a second sidecar writer. The two-instance
slice (`src/pbt/composed/two_instance_transport.rs`) drives `replicate_all`,
which installs no `OnPeerConnected` callback, so it has no admission writer to
race the revocation against. Closing this in the composed PBT needs the slice to
drive `share_subtree` / `accept_shared_subtree` and then revoke while a dial is
in flight — the same prerequisite as
`2026-09-09-a-refused-peer-is-still-remembered-and-later-dialed`. Pinned at the
crate level instead, deterministically, via a one-shot publish stall.

## Remedy

FIXED in `share-lifecycle` rev 3, in two parts, because each fixes a different
half and the first alone is a regression.

1. `SharedSnapshotStore::stage_tmp` gives every write a private tmp sibling
   (`<final name>.<pid>-<seq>.tmp`), and `publish_tmp` renames it. All four
   publish paths (`save`, `save_peers`, `save_port`, `save_generation`) share
   those two helpers instead of each repeating the tmp/fsync/rename sequence
   with its own fixed name. `sweep_stale_tmps` now collects any `*.tmp` in the
   directory, which is a widening: `stage_tmp` is the only producer of that
   suffix there.
2. `remember_peer` and `forget_peer_addrs` persist UNDER the `known_peers`
   write guard rather than after releasing it, so the memory mutation and the
   disk write move as one unit and the last writer to release the lock is the
   last writer to disk.

Covering tests, each red for the right reason first
(`lane-logs/r3-red-1.log`, `lane-logs/r3-red-2-fixA-only.log`; green in
`lane-logs/r3-green-1.log`):

- `holon_loro::shared_snapshot_store::tests::a_concurrent_peer_save_does_not_steal_this_writers_tmp_file`
  — red with the exact production ENOENT at the rename.
- `holon_loro::loro_share_backend::tests::a_concurrent_admission_does_not_fail_a_revocations_sidecar_write`
  — the R2-D1 signature at the backend level, red with the verbatim "could not
  be rewritten after revoking peer" message.
- `holon_loro::loro_share_backend::tests::a_concurrent_admission_cannot_restore_a_revoked_peers_addr_on_disk`
  — the silent half; green today by accident, RED once part 1 lands alone, green
  only with part 2.

Revocation state ordering is unchanged and is deliberately decided before
persistence, not rolled back: `revoke_share_peer` revokes in the live roster and
closes the enrollment window first, so a persistence failure still leaves the
peer un-dialable for the life of the process and returns `Err` naming the share
and the peer. The durability gap it reports is real and is the reason that
`Err` exists.

Related: `2026-09-09-a-refused-peer-is-still-remembered-and-later-dialed` (the
other writer of this sidecar).
