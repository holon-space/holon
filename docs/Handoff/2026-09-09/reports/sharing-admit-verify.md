# Adversarial verification — lane `sharing-admit`

pwd for every command: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sharing-admit`
Tree identity asserted: `@-` = `830d794f878f` (matches brief); `grep -q import_peer_delta crates/holon-loro/src/iroh_sync_adapter.rs` OK; `crates/holon-api/src/sharing.rs` exists.
Gate log (mine): `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/verify-sharing-admit-1788861362.log`

## VERDICT: CONFIRMED (with defects, all documentation/severity-framing or pre-existing-hole-adjacent; none refutes a claim)

---

## C1 — every peer-import ingress passes an admission decision — CONFIRMED
pwd: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sharing-admit`

Independent enumeration (`rg` over `crates/holon-loro/src`, `crates/holon-sharing/src`,
`crates/holon-app/src`, `crates/holon-loro-wiring/src` for
`.import(|import_batch|apply_update|import_peer_delta|from_snapshot|load_snapshot|import_with`).
`holon-app` and `holon-loro-wiring`: **zero** hits — no ingress there.

Network-reachable ingresses, all gated:
- `iroh_sync_adapter.rs:332` (initiator) and `:460` (acceptor) — the only two peer-byte
  imports; both call `import_peer_delta(doc, admitted, …)`. The raw `doc.import` at the
  two former sites is gone (confirmed by reading both functions in full).
- Entry points that mint the witness: `:255` `AdmittedPeer::dialed` (sync_doc_initiate),
  `:292` `dialed` (sync_doc_initiate_enrolled), `:386` `ungated` (sync_doc_accept),
  `iroh_advertiser.rs:365` `enrolled` / `:387` `ungated` (accept_loop). That is the
  complete set (`rg 'AdmittedPeer::(ungated|dialed|enrolled)'` → 5 non-test sites).
- Relay leg `holon-sharing/src/sync.rs:230-247` `pull_once` imports only inside
  `AdmitDecision::Import`, via `apply_update_with_origin(SYNC_IMPORT_ORIGIN, …)`.
  `acceptor.rs:189-215` enforces `required_capability` **inside** `admit`, so the
  `{ .. }` destructure is not a hole.

Non-network `.import` sites checked and each is own-device/local:
`shared_snapshot_store.rs:297` (owner's disk, corrupt file quarantined),
`shared_tree.rs:211/237/581` (local doc→doc extract into a FRESH doc),
`device_pairing_op.rs:745` (local replay of the staged doc, and it is already inside
`doc.with_write(|txn| …)` — the guard is held), `text_merge_provider.rs:148/162/175`
(local three-way merge scratch docs), `iroh_advertiser.rs:503/554` and
`iroh_sync_adapter.rs:514/519/746+` (inside `#[cfg(test)] mod tests` / the
`DirectSync`+`IrohSync` `SyncBackend` PBT harness — `IrohSync::sync_pair` at
`:542-575` goes through `sync_doc_accept`/`sync_doc_initiate` with an explicit
`Capabilities::read_write()`), `multi_peer.rs`, `import_atomicity_probe.rs`,
`deleted_container_purge.rs` (probes/tests).

Own-device paths carry an explicit `read_write` grant, each with the ruling named:
`container_registry.rs:239-254` (`start_share_gated` + `Capabilities::read_write()`,
D69.a/D72.a), `device_pairing_op.rs:696` / `:1066`.

No raw import reachable from a network peer was found.

## C2 — capabilities enforced in both directions — CONFIRMED
pwd: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sharing-admit`

Pinning tests exist and I ran them (my log, lines 343-346):
`peer_import::tests::a_read_only_peers_delta_is_refused_naming_the_missing_capability`
(also asserts the replica is left untouched),
`a_read_write_peers_delta_lands_tagged_sync_import`,
`a_write_only_admission_may_not_read`,
`an_admission_conferring_nothing_refuses_both_directions` — all PASS.
Relay-side equivalents exist in `crates/holon-sharing/src/acceptor.rs`
(`a_read_only_peers_write_into_the_owners_store_is_refused`,
`a_third_partys_read_only_chain_cannot_write_into_my_store`,
`a_read_write_peers_write_into_the_owners_store_is_admitted`) and passed in the same run.

Bypass probes, all negative:
- **Peer-supplied capabilities?** No. `share_enrollment.rs:879-911` `acceptor_enroll`
  reads only `EnrollmentProofMsg { capability_id, proof }` off the wire; the
  `Capabilities` value comes exclusively from the local `ShareAdmission` passed at
  `start_share_gated`/`start_share_ungated`. On the initiator side it is the local
  `grant` argument. Nothing capability-shaped is parsed from a peer.
- **`.ok()` / `unwrap_or(default)` on a capability parse?** None.
  `rg` over holon-loro/holon-sharing/holon-api for capability lines crossed with
  `.ok()|unwrap_or|_ =>|unwrap()` returns only `device_pairing_op.rs:1452-1458` — test
  asserts on `PairCapability::parse`, which `bail!`s on unknown input
  (`device_pairing_op.rs:139-145`) and has no default arm.
  `Capabilities` (`holon-api/src/sharing.rs`) has `Default` = the EMPTY set, and the
  empty set refuses both directions (pinned by
  `an_admission_conferring_nothing_refuses_both_directions`) — so `Default` fails closed.
- **`ShareAdmission::Ungated` constructors:** `rg 'ShareAdmission::Ungated'` →
  `iroh_advertiser.rs:139` (inside `start_share_ungated`, whose only callers are
  `iroh_advertiser.rs:448` and `:475`, both `#[cfg(test)]`) and
  `loro_share_backend.rs:1076` (inside `start_advertising_stable`). The lane's claim
  that `start_advertising_stable` is the one production site holds, and the `warn!` at
  `iroh_advertiser.rs:195-201` fires on every un-gated start. See gap G4 for the
  residual `AdmittedPeer::ungated` surface.

## C3 — red for the right reason — CONFIRMED
pwd: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sharing-admit`

`lane-logs/redgate-2956.log:572` = `CHECK_EXIT=0` (so the crate compiled; not a
compile-error red). Summary line: `5 tests run: 0 passed, 5 failed, 370 skipped`.
All five failures are behavioural assertions about admission/origin, e.g.
`peer_import.rs:240` `left: [""] right: ["sync_import"]` and
`iroh_sync_adapter.rs:771` `the initiator leg imported a peer delta under origin(s)
[""], none of them 'sync_import' — the import bypassed LoroDocument's write guard`.
No panic is unrelated; none is an `error[E…]`.

## C4 — gates — CONFIRMED
pwd: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sharing-admit`

Re-run by me under the build slot:
`bash ~/.claude/skills/orchestrator/scripts/with-build-slot.sh … cargo nextest run
-p holon-sharing -p holon-loro -p holon-loro-wiring --no-fail-fast`

- log line 1: pwd = the lane workspace (tree-identity assert inside the slot)
- log line 2: `nightly-2026-08-16-aarch64-apple-darwin (overridden by … /sharing-admit/rust-toolchain.toml)`
- log line 3: `NEXTEST_EXIT=0`
- final line: `Summary [ 14.968s] 461 tests run: 461 passed, 3 skipped`

Matches the lane's claimed 461. No "0 tests run". No pass-with-note signature was hit —
`iroh_advertiser::tests::gated_share_rejects_forged_ticket_serves_enrolled_peer` PASS
[14.735s]; `both_iroh_legs_tag_a_peer_delta_sync_import` PASS [1.381s] (line 212). No
iroh TIMEOUT, so no isolated rerun was needed. Two-instance composed PBT not re-run
(C1/C2 raised no doubt it would settle).

## C5 — bugfunnel entries — CONFIRMED with documentation defects
pwd: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sharing-admit`

Every covering test named by the two flipped entries exists and passed in my run
(verified by `rg -c "fn <name>"` per test, then by the PASS lines above):
6 named tests + `doc_lock::tests::two_wrappers_over_one_inner_doc_share_one_lock`.
The `Arc::as_ptr` lock-identity argument the entry rests on is real
(`doc_lock.rs` registry keyed by `Arc::as_ptr`, pinned by that test).

The NEW entry describes a real exposure — `start_advertising_stable`
(`loro_share_backend.rs:1045-1090`) is the only advertise call on the third-party
lifecycle and it passes `ShareAdmission::Ungated { read_write }`; its three callers are
`:1684` (share_subtree), `:1772` (accept_shared_subtree), `:2330` (restart rehydrate),
exactly as tabulated. `accept_loop` (`iroh_advertiser.rs:386-399`) then mints
`AdmittedPeer::ungated(read_write)` and runs the full sync — so a stranger reaching the
endpoint does get read+write. Confirmed.

**On the `shared_tree_id` severity question the brief asked me to settle:** it is
`Uuid::new_v4().to_string()` (`loro_share_backend.rs:1513`) — 122 random bits, so it is
NOT guessable. It is *disclosed*: it goes into the ticket, the mount node, projected SQL
rows, the QUIC ALPN (`loro-sync/{id}`, recoverable by an on-path observer from the QUIC
handshake), and into tracing fields on ~15 log lines. And `start_advertising_stable`'s
own doc comment states relay and discovery are disabled, so an attacker also needs direct
routability to the host. Correct severity: "anyone who obtains the id AND can route to
the host gets read+write, and revocation is inoperative" — not "anyone". See defect D3.

## C6 — nothing secret-shaped, no lax parse — CONFIRMED
pwd: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sharing-admit`

`jj diff --git` added lines scanned for
`(api[_-]?key|token|secret|password|passwd|bearer)\s*[:=]\s*"[A-Za-z0-9+/=_-]{16,}"` —
zero hits. `CapabilitySecret::generate()` appears only as a call, never a literal.
No `_ => default` or `.ok()` on an admit/capability parse (see C2).

---

## Defects

- **D1 — dangling file reference in the new bugfunnel entry.**
  `docs/Testing/bugfunnel/entries/2026-09-08-the-subtree-share-hot-path-advertises-un-gated.md`,
  Remedy section: "the advertiser now takes a `ShareAdmission`
  (`crates/holon-loro/src/peer_admission.rs`)". That file does not exist
  (`ls` → No such file). `ShareAdmission` is at `crates/holon-loro/src/iroh_advertiser.rs:56`;
  `AdmittedPeer` is at `crates/holon-loro/src/peer_import.rs:41`.

- **D2 — the same entry's root-cause line refs point at code the lane deleted.**
  It says `loro_share_backend.rs:1043-1073` "passes `None` for the advertiser's roster
  parameter" and that `iroh_advertiser.rs:288` is `if let Some(roster) = roster.as_ref()`.
  Post-lane, `loro_share_backend.rs:1076` passes `ShareAdmission::Ungated { capabilities:
  Capabilities::read_write() }`, and `iroh_advertiser.rs:351-388` is a `match &admission`
  with no `Option`. A reader following the entry after this lands finds neither.

- **D3 — the entry overstates the attacker precondition.** "the one thing an attacker
  needs is not a secret" — the id is a v4 UUID (`loro_share_backend.rs:1513`), i.e.
  unguessable-but-disclosed, and relay/discovery are off (per `start_advertising_stable`'s
  own doc comment), so routability is a second precondition. Neither qualification appears
  in the entry, which will mis-price the fix against other work.

- **D4 — a refused peer is still promoted to a dialable peer (inside the disclosed hole,
  but not disclosed).** `iroh_advertiser.rs:391-397`: the `on_peer_connected` callback
  fires *before* `sync_doc_handle_connection`, whose first act is
  `authorize_peer_read` (`iroh_sync_adapter.rs:421`). So a peer whose admission confers
  nothing — or which fails the whole sync — still has its `EndpointAddr` persisted by
  `LoroShareBackend::peer_connected_callback` (`loro_share_backend.rs:1026-1036`) into the
  known-peers sidecar, and `sync_with_peers` (`:1163`) later dials it granting
  `Capabilities::read_write()`. The capability refusal is therefore not fully one-way:
  refusing a peer's read does not stop us from later handing it a full-writer round.

## Gaps

- **G1 — no live-transport test of a capability refusal.** The Read-only/no-Write refusal
  is pinned only by `peer_import.rs` unit tests over a bare `Arc<LoroDoc>`. The one
  transport-level test the lane added
  (`both_iroh_legs_tag_a_peer_delta_sync_import`) asserts the *origin tag*, not the
  refusal, and every iroh test passes `Capabilities::read_write()`. A regression that
  dropped the capability check inside `import_peer_delta` would be caught; one that let a
  transport leg construct a `read_write` admission where the share said `read_only` would
  not.

- **G2 — no invariant that a backend-advertised share is gated.** Acknowledged by the new
  entry (ORACLE secondary), and still true after the lane: nothing fails if a future
  advertise site writes `Ungated`. The safeguard is `rg` + a `warn!`, not a test.

- **G3 — the export direction is authorized but not lock-sealed.** Lane assumption 5:
  `authorize_peer_read` gates the export, but the export itself is still a raw
  `doc.export` outside `with_read`. No test covers this seam.

- **G4 — the ungated surface is wider than the stated worklist.**
  `AdmittedPeer::ungated` and `sync_doc_accept` are both `pub` out of `holon-loro`
  (`iroh_sync_adapter.rs:1028`, `peer_import.rs:88`). A future production caller can mint
  an un-gated admission without ever writing `ShareAdmission::Ungated`, so the entry's
  "`rg 'ShareAdmission::Ungated'` is the exact worklist" would silently under-report.
  Today only tests and the `IrohSync` PBT backend use them.

- **G5 — capabilities are per-share, not per-peer** (lane assumption 3). Correct as
  scoped, but it means a roster with a mix of own-device and third-party members cannot
  express different grants; not exercised by any test because no such roster exists yet.

---

# Rev 2

pwd for every command: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sharing-admit`
`@-` still `830d794f878f`, still uncommitted. Diff grew 1088→1359 insertions; same 16 files
(no new file added — see D5).

## VERDICT (rev 2 delta): CONFIRMED

### D4 — fixed by construction. CONFIRMED
pwd: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sharing-admit`

- `peer_import.rs:182-201` — `PeerReadAccess { admitted: AdmittedPeer }`, field **private**,
  no public constructor, no `Default`. The only construction site in the whole crate is
  `peer_import.rs:210` inside `authorize_peer_read` (`rg 'PeerReadAccess'` → 8 hits, all
  either the definition, that construction, or a `&PeerReadAccess` parameter/import).
  `Clone` is derived, but cloning still requires already holding one.
- `iroh_advertiser.rs:92` — `OnPeerConnected = Arc<dyn Fn(&PeerReadAccess, EndpointAddr)>`.
  The callback can no longer name a container without the witness (`access.container()`).
- **The witness is the only path to `remember_peer` from a network dialer.**
  `remember_peer` (`loro_share_backend.rs:993`) is private; every caller enumerated:
  `:986` `remember_admitted_peer(&self, access: &PeerReadAccess, …)` — witness-gated, and the
  only caller of *that* is the callback at `:1041-1047`;
  `:1776` `accept_shared_subtree` remembering the **ticket author's own** addr (a local value
  from the ticket, not a dialer); `:3884` a `#[cfg(test)]` fixture. No other caller exists.
- **Ordering fixed.** `iroh_advertiser.rs:400-413`: `authorize_peer_read` now runs in the
  accept loop *before* `connection_remote_addr` + the callback (`:421` `cb(&access, remote)`),
  with the reason stated in the code. On refusal it returns without ever building the addr.
- **Close code.** `iroh_advertiser.rs:410` — the capability refusal closes with
  `ENROLLMENT_REFUSED_CODE.into(), b"capability refused"`, the same code the enrollment
  refusal uses at `:387`. CONFIRMED as specified.
- Also verified the read gate cannot be skipped anywhere: `sync_doc_handle_connection`
  (`iroh_sync_adapter.rs:429-432`) now takes `&PeerReadAccess`, so both its callers
  (`sync_doc_accept:388-391` and the accept loop) must pass `authorize_peer_read` to compile.

### Red-first for the two new tests — CONFIRMED
`lane-logs/rev2-redgate-48970.log:108` `Summary [2.444s] 2 tests run: 0 passed, 2 failed,
375 skipped`. Both reds are behavioural, not compile:
- `:84` `a peer admitted with NO capabilities was remembered for container(s)
  ["refused-read"] — 'sync_with_peers' will dial it back granting read+write, so the refusal
  held for one round only` — the exact D4 mechanism.
- `:103` `a read-only peer's delta must not be imported over the live transport: ()`.

Both tests read correctly: `a_peer_refused_for_read_is_never_remembered_as_a_known_peer`
(`iroh_advertiser.rs:588-618`) carries a **control case** (`admitted-read` +
`read_write` must still be remembered), so an empty recording cannot be a broken dial;
`a_read_only_peers_write_over_iroh_is_refused_as_peer_access_refused`
(`iroh_sync_adapter.rs:799-860`) runs a real iroh round, downcasts through the error
`chain()` to `PeerAccessRefused`, asserts `missing == Capability::Write`, AND asserts the
acceptor's replica does not contain the initiator's text — so it pins refusal, not failure.

### D1 / D2 / D3 — CONFIRMED corrected
- **D1** — `peer_admission.rs` is gone from the entry; it now cites
  `crates/holon-loro/src/iroh_advertiser.rs:58`, which is `pub enum ShareAdmission`. Correct.
- **D2** — root cause now cites `loro_share_backend.rs:1059` (= `async fn
  start_advertising_stable`) and `iroh_advertiser.rs:360-398` (= the `match &admission`
  block). Both resolve to the described code in the current tree.
- **D3** — the entry now carries an explicit "**Attacker preconditions — two, not one**"
  paragraph: `Uuid::new_v4()` at `:1527` (verified: that line is
  `let shared_tree_id = Uuid::new_v4().to_string();`), 122 bits, not guessable; disclosed via
  ticket / mount node / SQL rows / QUIC ALPN / ~15 tracing fields; and relay+discovery
  disabled so routability is required. Matches what I measured independently.

### Gates — CONFIRMED (run by me)
pwd asserted inside each slot as `TREE=`/`pwd` line 1 of each log.

1. `cargo nextest run -p holon-sharing -p holon-loro -p holon-loro-wiring --no-fail-fast`
   log `…/scratchpad/verify-sharing-admit-rev2-1788864499.log`
   - line 2: `nightly-2026-08-16-aarch64-apple-darwin (overridden by … /sharing-admit/rust-toolchain.toml)`
   - line 3: `NEXTEST_EXIT=0`
   - line 532: **`Summary [ 14.706s] 463 tests run: 463 passed (1 leaky), 3 skipped`** — the
     expected 463.
   - line 221 `PASS a_read_only_peers_write_over_iroh_is_refused_as_peer_access_refused`;
     line 255 `PASS a_peer_refused_for_read_is_never_remembered_as_a_known_peer`.
   - line 526: `LEAK [4.199s] loro_share_backend::tests::shared_block_write_routes_to_shared_doc_and_syncs`
     — passed but leaked a handle at exit. Not in the lane's stated pass-with-note list, and
     the lane's own `rev2-greengate` claim of 463 does not mention it. See defect D6.

2. Isolated two-instance composed PBT, which the lane skipped:
   `timeout 900 cargo nextest run -p holon-integration-tests --features pbt --test
   two_instance_composed_pbt --no-fail-fast --test-threads 1`
   log `…/scratchpad/verify-sharing-admit-rev2-twoinst-1788865381.log`
   - **`Summary [ 525.575s] 29 tests run: 29 passed (1 slow), 3 skipped`**
   - `TWOINST_EXIT=0`, `TWOINST_WALL=618s` (under the 900 s budget)
   - The lane's justification for skipping is sound and I verified it:
     `container_registry.rs:250-252` `replicate_all` passes `on_peer_connected: None`, so the
     witness-typed callback is not on that path; the only source change reaching the slice is
     the extra `Capabilities::read_write()` argument at `two_instance_transport.rs:467-476`.
     The run confirms it: nothing regressed, including
     `production_pairing_replicates_the_whole_store_over_iroh` and
     `post_boot_create_reaches_the_receiver_over_iroh`.

## Rev 2 defects

- **D5 (process) — the D4 bug was fixed with no bugfunnel entry.** Project CLAUDE.md: "Every
  bug discovered OUTSIDE an automated test (dogfooding, agent exploration, user report) MUST
  be triaged with the `bug-gap-triage` skill and recorded as ONE new file under
  `docs/Testing/bugfunnel/entries/`". D4 was found by adversarial verification, not by a test,
  and it was a real behaviour bug (a refused peer became a dialable known peer that
  `sync_with_peers` would later grant `read_write`). `jj diff --name-only | rg bugfunnel`
  returns the same three files as rev 1 — no new entry. `rg` for either new test name or
  `PeerReadAccess` across `docs/Testing/bugfunnel/entries/*.md` returns nothing, so neither
  flipped entry names the two new covering tests either, and the ledger count is unchanged
  for a bug that was found and fixed.

- **D6 (evidence) — an unreported LEAK in the gate the lane cites as clean.**
  `loro_share_backend::tests::shared_block_write_routes_to_shared_doc_and_syncs` is reported
  `LEAK` by nextest in my run (log line 526; summary "463 passed (**1 leaky**)"). The lane
  reports the same gate as `461/463 passed` with no leak note, and this test is not in the
  documented pass-with-note signatures. It passes, so it does not refute the gate, but a
  leaked handle in a share-backend test that this lane changed the callback signature of
  should be accounted for rather than silently absorbed.

## Rev 2 gaps (rev-1 gaps that remain)

- **G1 is now CLOSED** — `a_read_only_peers_write_over_iroh_is_refused_as_peer_access_refused`
  is exactly the live-transport capability-refusal test rev 1 said was missing.
- **G2 still open** — no invariant asserts "every backend-advertised share is gated"; the
  safeguard remains `rg 'ShareAdmission::Ungated'` plus a `warn!`.
- **G3 still open** — `authorize_peer_read` gates the export, but the export itself is still a
  raw `doc.export` outside `with_read`.
- **G4 narrowed but open** — `sync_doc_handle_connection` now requires the witness, so the
  read gate is unbypassable. `AdmittedPeer::ungated` and `sync_doc_accept` remain `pub`
  (`iroh_sync_adapter.rs:1115`), so a future production caller can still mint an un-gated
  admission without writing `ShareAdmission::Ungated`, and the entry's "`rg` is the exact
  worklist" would under-report it.
- **G5 unchanged** — capabilities are per-share, not per-peer.
