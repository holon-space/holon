---
id: 2026-11-02-compacted-peer-refuses-concurrent-edits
date: 2026-11-02
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A peer that reloaded its history-compacted snapshot refuses every edit a paired peer made concurrently to the trim, and the other peer silently does not receive the compacted peer's edits, so the two never converge.
---

## Bug
Found by the type-token spike (round 3, experiment E1), not by a regular test.
Pinned by `crates/holon-loro/tests/compacted_peer_receives_concurrent_edits.rs`
(`every_edit_reaches_every_peer_with_history_compaction`).

- Peer A restarts once while apart from peer B. A's first save of the session was a shallow snapshot, so after the reload A's document is shallow.
- B's ops are concurrent to A's trim point. B's schema init alone is enough: A restarted with no edits and B made one edit, and B's edit never arrives (shrunk case in the test).
- A dialing B: A's import fails with `Import Failed: The dependencies of the importing updates are not included in the shallow history of the doc` (`ImportUpdatesThatDependsOnOutdatedVersion`). The dial returns that error.
- B dialing A: A sends its shallow snapshot. B's import returns Ok but B does not get A's concurrent edits. Then A refuses B's delta and drops the connection; B sees `connection lost`.
- Every later sync repeats both failures. Each author keeps its own edits.
- In one run, a generated case made Loro panic in the import (an `unwrap` in the pending-changes module of the `loro-internal` crate), and a later sync hit `poisoned LoroMutex`. The case is not pinned yet.

## Root cause
- The first snapshot of every store session is a shallow snapshot at the doc's frontier: `crates/holon-loro/src/loro_document_store.rs:491-500` (`n.is_multiple_of(64)` holds for n = 0), export at `crates/holon-loro/src/loro_document.rs:403`.
- The trim does not wait for paired peers to acknowledge the frontier. A shallow doc refuses every change that is concurrent to its shallow root (Loro `decode_oplog` in the `loro-internal` crate).
- `export_delta_or_full_snapshot` (`crates/holon-loro/src/iroh_sync_adapter.rs:84`) covers only a receiver that is behind the sender's trim. It sends the shallow snapshot, which a receiver with concurrent history cannot merge.
- Disclosure: no condition. The initiator returns the error (`iroh_sync_adapter.rs:346`); the acceptor logs `warn!` (`crates/holon-loro/src/iroh_advertiser.rs:488`). The direction where B imports A's snapshot reports success.
- Production reach: the root container syncs over iroh through `ContainerRegistry::replicate_all` (`crates/holon-loro/src/container_registry.rs:237`), which only the keystone two-instance slice calls today. Share docs, which production syncs, are saved as full snapshots. So the defect is latent for the vault document and live for any continuous own-device sync.

## Missing piece
- COVERAGE: the keystone two-instance iroh slice (`crates/holon-integration-tests/src/pbt/composed/two_instance_transport.rs`) runs root-container sync but has no restart transition for a side, so no generated sequence reloads a shallow snapshot between syncs.
- ENVIRONMENT (secondary): the compaction is visible only after a reload. A side that never restarts keeps its full in-memory history.

## Litmus answers
- Is there a transition sequence in the current catalog and wiring that reaches this state? No: the two-instance slice has no per-side restart.
- Does the failing code path run in the keystone's wiring? Yes, the sync path does (`replicate_all`, `sync_doc_initiate_enrolled`). The shallow reload does not.

## Remedy
FIXED in change zqnkqrqo.
- The store keeps each paired peer's last version vector (`crates/holon-loro/src/peer_versions.rs`), persisted across a restart. The sync handshake carries the sender's version and its map device -> version; the receiver merges per device the larger vector.
- The shallow snapshot trims only below the meet of the own version and every recorded vector. A store with no recorded peer does not trim.
- A refused import raises a `PeerUpdateRefused` condition that names the peer and the Loro error. A peer with the old protocol is refused at the handshake with a `SyncProtocolMismatch` condition.
- Pairing with an owner whose history is trimmed fails visibly and changes nothing.
- Pinned by `crates/holon-loro/tests/compacted_peer_receives_concurrent_edits.rs` (pair, star, chain and 3-cycle topologies with restarts; safety: no refusal; liveness: the trim floor advances) and `crates/holon-loro/tests/pairing_refuses_trimmed_owner.rs`.
- Open: a late join of a device that did not pair by adoption (#927).
