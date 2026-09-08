---
id: 2026-09-08-the-subtree-share-hot-path-advertises-un-gated
date: 2026-09-08
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  `LoroShareBackend` advertises every third-party subtree share with no
  enrollment roster, so the H5 capability gate never runs on the path that
  ships — anyone who obtains the (unguessable but widely disclosed)
  `shared_tree_id` AND can route to the host both reads the shared subtree and
  writes arbitrary CRDT ops into it, with no revocation.
---

## Bug

Found by code audit in the `sharing-admit` lane on 2026-09-08 while enumerating
every peer-import ingress for D86.a. Not a runtime failure: the gate simply is
not wired on the production path, and no test drives that path.

`share_subtree` mints a `CapabilitySecret`, puts it in the ticket, and hands the
recipient a share. The recipient proves nothing, because the advertiser was
started without a roster.

| Path | Advertise call | Roster passed | Peer must prove a capability |
|---|---|---|---|
| own-device `replicate_all` | `start_share_gated` | yes | yes |
| `share_subtree` (author) | `start_advertising_stable` | **none** | **no** |
| `accept_shared_subtree` (recipient) | `start_advertising_stable` | **none** | **no** |
| share rehydrate after restart | `start_advertising_stable` | **none** | **no** |

## Root cause

`crates/holon-loro/src/loro_share_backend.rs:1059` —
`start_advertising_stable` advertises with no enrollment, with the reason stated
in the code: the enrollment gate is "not yet flipped ON in the backend hot
path". Post-`sharing-admit` that is the explicit
`ShareAdmission::Ungated { capabilities: Capabilities::read_write() }` at
`:1090-1092`; before the lane it was an unmarked `None` roster argument. Its
three callers are the whole third-party share lifecycle: `share_subtree`
(`:1698`), `accept_shared_subtree` (`:1786`) and the restart rehydrate
(`:2344`).

`crates/holon-loro/src/iroh_advertiser.rs:360-398` is where the admission is
taken: the `Ungated` arm (`:395`) mints an `AdmittedPeer` for whoever dialled,
with no proof, so the accept loop runs the full sync protocol — which both
exports our state to the peer and imports its delta into our doc.

The ticket's capability is therefore decorative on both ends. The second
disclosure is at `:1750-1754`: "possession of this ticket string remains a
bearer read+write capability".

**Attacker preconditions — two, not one.** The `shared_tree_id` is
`Uuid::new_v4()` (`:1527`), 122 random bits: it is NOT guessable. It IS
disclosed, in the ticket, in the mount node, in projected SQL rows, in the QUIC
ALPN (`loro-sync/{id}`, readable off the handshake by an on-path observer) and
in tracing fields on ~15 log lines. And `start_advertising_stable` disables
relay and discovery, so the attacker must also be able to route to the host
directly. The correct reading is therefore *anyone who obtains the id AND can
route to the host*, not *anyone* — but obtaining the id takes no cryptanalysis,
only one of the disclosure channels above.

Consequences, in order of severity:

1. **Un-authenticated write.** A peer meeting both preconditions imports
   arbitrary ops into the shared subtree. Loro merges them; the projection
   writes them to SQL and the org write-back puts them on disk.
2. **Un-authenticated read.** The same peer receives the whole shared subtree
   in the acceptor's delta before it has proved anything.
3. **Revocation is inoperative.** Unsharing removes nothing an attacker holds,
   because it never held a capability to revoke.

## Missing piece

**ENVIRONMENT (primary).** The enrollment mechanism is proved only where it is
constructed directly: every gated-share test in `iroh_advertiser.rs` builds its
own `IrohAdvertiser` and passes a roster explicitly. No test drives
`share_subtree` / `accept_shared_subtree` over a live transport, so the wiring
that ships — the un-gated one — has no test-side existence at all. The gate is
proven in a wiring nobody runs.

**ORACLE (secondary).** Nothing asserts the negative even where the path is
reachable: no invariant says "every container advertised by the backend is
gated by a roster", so an un-gated advertise would not be flagged by a case that
hit it.

## Keystone repro

The keystone (`tests/general_e2e_composed_pbt.rs`) boots one instance and has no
second peer, so it cannot reach this. The two-instance slice
(`src/pbt/composed/two_instance_transport.rs`) drives `replicate_all`, which is
the *gated* path — it exercises the branch that is already correct. Reproducing
this needs a slice that drives `share_subtree` + `accept_shared_subtree` over
iroh and then dials the author from an un-enrolled stranger endpoint.

## Remedy

OPEN. Out of scope for the `sharing-admit` lane, which owns the import-side
admission (D86.a) rather than the share lifecycle.

What this lane DID do is make the hole typed and greppable instead of a `None`:
the advertiser now takes a `ShareAdmission`
(`crates/holon-loro/src/iroh_advertiser.rs:58`), and the un-gated callers must
name themselves `ShareAdmission::Ungated { .. }` and say which capabilities they
hand a stranger. Every un-gated share start also logs a warning naming the
container. The hole is unchanged in effect — a stranger still gets read+write —
but it can no longer be reached by accident and `rg 'ShareAdmission::Ungated'`
lists every site that must be converted. (That grep is the worklist for the
*share* side only; `AdmittedPeer::ungated`
(`crates/holon-loro/src/peer_import.rs`) is also `pub`, so a future caller could
mint an un-gated admission without writing `ShareAdmission::Ungated`. Today only
tests and the PBT `SyncBackend` do.)

Fix shape, when the lifecycle lane takes it:

1. `share_subtree` mints the `CapabilitySecret` BEFORE advertising and passes a
   `ShareRoster` built from it (the mint already exists at `:1716`, just after
   the advertise instead of before).
2. `accept_shared_subtree` builds its roster from the ticket's capability, and
   its dial switches from `sync_doc_initiate` to `sync_doc_initiate_enrolled`.
3. The capability secret is persisted (keychain) so the restart rehydrate at
   `:2344` can rebuild the roster — `roster_sidecar.rs` already has
   `ShareRoster::rehydrate` for exactly this.
4. `sync_with_peers` (`:1131`) enrolls on each dial with that persisted secret.

Related: `2026-09-02-peer-imports-bypass-the-loro-document-write-guard` (the
import-side half, fixed by this lane) and
`2026-09-02-capability-write-is-enforced-nowhere`.
