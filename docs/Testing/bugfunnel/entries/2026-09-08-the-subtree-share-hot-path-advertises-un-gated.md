---
id: 2026-09-08-the-subtree-share-hot-path-advertises-un-gated
date: 2026-09-08
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
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

FIXED. The share LIFECYCLE now runs through the H5 gate end to end, and a
production path can no longer even NAME the un-gated admission: the
`ShareAdmission::Ungated` variant, `start_share_ungated`, `AdmittedPeer::ungated`
and the roster-less `sync_doc_accept` are `#[cfg(test)]` / `#[cfg(any(test,
feature = "test-helpers"))]` and `pub(crate)`. The check is `cargo check -p
holon-loro --lib`, not a grep — a production call site fails with
`error[E0599]: no variant named 'Ungated' found for enum 'ShareAdmission'`.

The `sharing-admit` lane made that possible first: it replaced the advertiser's
`Option<SharedRoster>` with a typed `ShareAdmission`, so every un-gated caller
had to name itself and say which capabilities it handed a stranger. That turned
the hole from a `None` into an enumerable worklist; this lane converted the
worklist and then compiled the variant out.

Four steps, all in `crates/holon-loro`:

1. `share_subtree` mints the `CapabilitySecret` BEFORE advertising, files it in
   the keychain, builds a `ShareRoster` from it and advertises
   `ShareAdmission::Enrolled` (`loro_share_backend.rs`, `install_roster`).
2. `accept_shared_subtree` builds its own roster from the TICKET's capability,
   advertises gated, and dials with `sync_doc_initiate_enrolled`. It pins the
   author it dialed (`ShareRoster::pin_dialed`), so the author's later inbound
   dials need no fresh enrollment even outside the ticket's window.
3. The capability secret is persisted to the OS keychain (`share_credentials.rs`
   — a new `ShareCredentials` over `holon_secrets::KeychainStore`), and the
   pinned-peer set to the owner-signed `shares/<id>.roster.json` sidecar. The
   restart rehydrate rebuilds the roster from the two and REFUSES to advertise a
   share whose roster it cannot rebuild — degraded and disclosed
   (`ShareDegraded::RehydrationFailed`), never silently un-gated.
4. `sync_with_peers` dials with the persisted capability, and `unshare` drops it
   (`forget_capability`), which makes every ticket ever issued for that share
   inert. `revoke_share_peer` revokes ONE peer: it un-pins it, closes the
   enrollment window so the capability it kept cannot re-admit it, and forgets
   its dial addrs so this device's own pull cannot bring its ops in either.

Covering tests (the first, third and fourth over the LIVE iroh transport):

- `holon_loro::loro_share_backend::tests::a_stranger_holding_only_the_shared_tree_id_can_neither_read_nor_write_the_share`
  — consequences 1 and 2, driven through `share_subtree` itself, which is the
  wiring this entry says no test reached.
- `holon_loro::iroh_advertiser::tests::a_read_only_gated_share_serves_an_enrolled_peer_but_refuses_its_delta`
  — membership and capability are separate answers; the refusal is the typed
  `PeerAccessRefused` naming the missing `Write`.
- `holon_loro::loro_share_backend::tests::revoking_a_peer_stops_every_further_import_from_it`
  — consequence 3.
- `holon_loro::loro_share_backend::tests::a_restart_reloads_the_roster_so_an_enrolled_peer_need_not_re_enroll`
  — the persistence that makes the gate survivable.
- `holon_loro::share_credentials::tests::a_missing_capability_is_a_loud_err_not_an_absent_value`
  and siblings — a missing secret fails closed instead of reporting "no secret".
- `holon_loro::loro_share_backend::tests::a_share_whose_roster_cannot_be_rebuilt_is_not_advertised`
  — the fail-closed direction itself: a lost keychain entry stops the share
  being served rather than downgrading it, and the refusal is disclosed. Carries
  its own control (the same restart WITH the keychain does advertise, gated).
- `holon_loro::loro_share_backend::tests::a_revoked_peer_is_not_re_dialed_after_a_restart`
  and `…::a_revocation_that_cannot_be_persisted_fails_loudly_naming_the_peer`
  — the OUTBOUND half of revocation survives a relaunch, and a revocation whose
  sidecar write fails is an `Err` rather than a warning the caller cannot see.

Residual, stated rather than closed: the ticket remains a bearer credential for
the length of its enrollment window, and admission still rests on iroh's
QUIC/TLS peer authentication (`share_enrollment`'s own disclosure). What changed
is that the disclosed `shared_tree_id` is no longer sufficient, and that the
window, the peer cap and revocation now bound what a leaked ticket can do.

Related: `2026-09-02-peer-imports-bypass-the-loro-document-write-guard` (the
import-side half), `2026-09-02-capability-write-is-enforced-nowhere` and
`2026-09-09-a-refused-peer-is-still-remembered-and-later-dialed`.
