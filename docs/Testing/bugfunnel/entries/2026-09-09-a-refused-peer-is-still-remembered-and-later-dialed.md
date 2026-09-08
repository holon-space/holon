---
id: 2026-09-09-a-refused-peer-is-still-remembered-and-later-dialed
date: 2026-09-09
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  The advertiser handed an inbound dialer's address to the remember-this-peer
  callback BEFORE any capability decision, so a peer the admission refused was
  persisted to the known-peers sidecar and dialed back on the next
  `sync_with_peers` round granting read+write — the refusal held for one round
  only.
---

## Bug

Found by the adversarial verifier pass on the `sharing-admit` lane (rev 1),
2026-09-09 — outside any test, by reading the accept loop against the
persistence path it feeds. Filed as defect D4 of that verification and fixed in
rev 2 of the same lane.

The capability refusal that lane added was not one-way. Refusing a peer's read
stopped that round's sync and nothing else: the peer's `EndpointAddr` had
already been written to the share's known-peers sidecar, and THIS device dials
every addr in that sidecar itself, granting `Capabilities::read_write()`. So a
peer admitted with no capabilities — including a peer of an `Ungated` share the
owner had narrowed — got a full-writer round on the next sync anyway, and kept
getting one after a restart, because the sidecar is persisted.

## Root cause

Line numbers are the PRE-fix (rev 1) tree; the post-fix locations follow.

`crates/holon-loro/src/iroh_advertiser.rs:391-397` — the accept loop captured
`connection_remote_addr(&conn)` and fired the `OnPeerConnected` callback, and
only THEN called `sync_doc_handle_connection`, whose first act was
`authorize_peer_read` (`crates/holon-loro/src/iroh_sync_adapter.rs:421`). The
ordering was invisible because the two halves live in different modules: the
gate was in the sync leg, the persistence was in the accept loop above it.

`crates/holon-loro/src/loro_share_backend.rs:1026-1036`
(`peer_connected_callback`) is what that callback does — `remember_peer`, which
merges the addr into `known_peers` and saves the sidecar. `:1163`
(`sync_with_peers`) then dials every addr in that sidecar with
`Capabilities::read_write()`.

The callback's own type was the enabling condition:
`OnPeerConnected = Fn(String, EndpointAddr)` could be called for any peer at
all, because it took a bare container name — nothing in the signature required
a decision to have been taken.

## Missing piece

**ENVIRONMENT (primary).** The failing wiring does not exist in any test. The
callback is installed by exactly one caller, `LoroShareBackend`
(`start_advertising_stable`), and no test drives that over a live transport;
every test that builds an `IrohAdvertiser` directly passes
`on_peer_connected: None`, and the own-device path (`replicate_all`,
`container_registry.rs`) passes `None` as well. The one live refusal test that
did exist — `gated_share_rejects_forged_ticket_serves_enrolled_peer` — refuses
at ENROLLMENT, before an `AdmittedPeer` exists, and installs no callback, so it
could never observe the persistence half.

**ORACLE (secondary).** Nothing asserted the negative either: no invariant said
"a peer the admission refused is never recorded as a peer this device dials",
so a case that reached the state would still have passed.

## Keystone repro

The keystone (`tests/general_e2e_composed_pbt.rs`) boots one instance and has
no second peer, so it cannot reach this. The two-instance slice
(`src/pbt/composed/two_instance_transport.rs`) drives `replicate_all`, which
installs no callback and grants `read_write` — it exercises neither half. The
gap is closed at the crate level instead (below); a composed repro would need
the slice to drive `share_subtree` / `accept_shared_subtree` with a narrowed
grant, which is the same prerequisite as
`2026-09-08-the-subtree-share-hot-path-advertises-un-gated`.

## Remedy

FIXED in `sharing-admit` rev 2, by type rather than by ordering alone.
`authorize_peer_read` now returns a `PeerReadAccess` witness
(`crates/holon-loro/src/peer_import.rs`), and `OnPeerConnected` is
`Fn(&PeerReadAccess, EndpointAddr)` — the witness is the only source of the
container name in that signature, so an unauthorized dialer cannot be handed to
the persistence callback at all. `sync_doc_handle_connection` takes the witness
too, so the read gate is passed before the sync leg is reachable rather than
inside it, and the accept loop closes a refused connection with
`ENROLLMENT_REFUSED_CODE` before anything is remembered. On the backend,
`remember_admitted_peer(&PeerReadAccess, addr)` is the inbound-dialer entry
point; the raw `remember_peer` is private with its two bases named.

Covering tests (both red-for-the-right-reason first, `lane-logs/rev2-redgate-48970.log`):

- `holon_loro::iroh_advertiser::tests::a_peer_refused_for_read_is_never_remembered_as_a_known_peer`
  — a live iroh dial into a share that admits with no capabilities records
  NOTHING; the control case with `read_write` records exactly one.
- `holon_loro::iroh_sync_adapter::adapter::tests::a_read_only_peers_write_over_iroh_is_refused_as_peer_access_refused`
  — the sibling direction over the live transport, so the typed refusal is
  pinned end to end and not only over a bare `Arc<LoroDoc>`.

The residual is disclosed, not fixed: the callback still fires before the sync
round COMPLETES, so an ADMITTED peer whose round then fails is still
remembered. That is deliberate — `conn.paths()` empties once the connection
drops, so a later callback would lose the addr for exactly the flaky peers the
sidecar exists for. It is a liveness cost, not an authorization one.

Related: `2026-09-08-the-subtree-share-hot-path-advertises-un-gated` (the share
that admits the stranger in the first place, still OPEN),
`2026-09-02-peer-imports-bypass-the-loro-document-write-guard` and
`2026-09-02-capability-write-is-enforced-nowhere` (both fixed by rev 1 of the
same lane).
