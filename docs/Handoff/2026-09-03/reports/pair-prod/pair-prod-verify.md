# pair-prod — fresh-context security verification

**Verdict: CONFIRMED** (lane claim reproduces; teeth bite) — with security findings below, one of them a
data-loss defect that should block landing.

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/pair-prod`, uncommitted diff on `integration ff7448cc`,
9 files / 865 insertions. `pair_with_owner` present on disk at `crates/holon-loro/src/device_pairing_op.rs:346`.
Tree restored byte-identical after both teeth checks (sha256 verified against pre-edit backups; `cp` only, no `jj restore`).

## 1. Re-run gates

| Gate | Result |
|---|---|
| `nextest -p holon-loro -p holon-sharing -p holon-architecture-tests` | **403 run, 403 passed**, 3 skipped (= lane's 329+67+7) |
| `loro_doc_escapes_match_the_allow_list` | PASS |
| `two_instance_composed_pbt` (once) | **14 passed**, 7 skipped |
| `cargo check -p holon-gpui` | clean (2 pre-existing warnings) |

`holon-app` (164) and keystone-smoke (4) were outside the assigned re-run set and were **not** re-verified.

### Allow-list entries the lane added

One new row, `("crates/holon-loro/src/device_pairing_op.rs", 2)`. Both escapes are `container.doc.doc()`:

- `:311` — handing the doc to `IrohAdvertiser::start_share_gated`, which retains it for the share's life.
- `:387` — handing the doc to `sync_doc_initiate_enrolled` for the length of the dial.

**Judgement: both necessary.** The iroh transport API is typed on `Arc<LoroDoc>`; there is no borrow-scoped
alternative, and both sites mirror existing allow-listed rows (`container_registry.rs`, `loro_backend.rs`). The
count is exact, so the arch test still has teeth against a third escape.

## 2. Teeth

| Mutation | Expected | Observed |
|---|---|---|
| Remove `device` provider registration (`loro_module.rs`) | replication tests RED on "No provider registered for entity: device" | **RED** — all 3 `production_pairing*` tests fail; oracle text verbatim: `No provider registered for entity: device (operation: 'pair_offer')` |
| Remove the mounts refusal (`pair_with_owner`) | mount test RED | **RED, isolated** — `production_pairing_refuses_a_receiver_that_holds_mounts` fails ("pairing a receiver that holds a mount SUCCEEDED"); the other two stay green |

Both mutations bite, and the second is correctly isolated. The tests are not vacuous.

## 3. Security findings (severity-ranked)

### S1 — HIGH — Content under `block:journals` defeats `ReceiverNotEmpty`; pairing proceeds over user data

`blocks_outside_the_app_seeded_families` (`device_pairing_op.rs:250-280`) grows the seeded set to a **fixed point
over `parent_id`** from the two roots `block:__default__` and `block:journals`, then reports only blocks outside
that closure.

A day block's parent is `block:journals`; a user note typed into that day block has the day block as parent. Both
land in `seeded`, so the function returns `[]`, `ReceiverNotEmpty` never fires, and the pair proceeds — exactly the
case the refusal exists to stop. Journals is the boot landing page (`general_e2e_composed_pbt.rs:451`: "boot seeds
focus on `block:journals`"), and the lane's own harness doc says a fresh receiver already holds "the day block its
rule mints" (`two_instance.rs`, `boot_two_instances_with_an_empty_receiver_on`). A jotted journal note is therefore
*the* most likely content on a nearly-fresh device, and it is precisely what the guard exempts.

Failure scenario: user jots a note into today's journal on the new phone, pairs it, and the note is inside the
"app-seeded" closure — the guard reports the device empty and the store is adopted over it.

Aggravating: **no test covers `ReceiverNotEmpty` at all.** The two new tests cover replication and the *mounts*
refusal only. The guard the lane calls "the whole grep" for D78.d is untested.

*Confidence: read-confirmed from the closure algorithm + the harness's own seeding comment; not run-confirmed —
I did not build a receiver with a journal child.*

### S2 — HIGH — A `read` invite is accepted and returns success, but grants full WRITE to the whole store

The base64 invite carries one `Ticket` per container in `replication_set()`, and each ticket embeds a full 32-byte
`CapabilitySecret` in plaintext JSON. Possession of the invite string therefore grants enrollment into **every**
container — the user's entire replicated store.

There is no read/write dimension anywhere on that path: `acceptor_enroll` proves possession of a `CapabilitySecret`
and nothing else; the rule lives in `holon_sharing::acceptor::admit`, which has no caller here. The lane discloses
this (module doc `:15-21`, and the `#[ignore]`d D86 test).

The defect is not the gap itself but the **response to it**: `pair_offer` accepts `capability: "read"`, mints the
invite, and returns success — presenting a grant it cannot honor. Per CLAUDE.md's priority order that is case 4
(silently degrades to look fine) where case 3 (refuse loudly) is available: `PairCapability::Read` should be
refused at `pair_offer` until D86 lands.

### S3 — MEDIUM — One invite pairs four devices, invisibly to the owner

`INVITE_MAX_PEERS = 4`, and nothing marks an invite consumed after a successful pair. Within the 15-minute TTL,
**four distinct QUIC identities can each enroll** on the same invite and each adopt the whole store.

Observability: the only signal is `debug!("[advertiser:{id}] peer enrolled (newly={})")`
(`iroh_advertiser.rs:293-296`), normally filtered out. `pair_offer` passes `on_peer_connected: None`, so no callback
fires. There is no UI surface and no way to enumerate paired devices. The `share_enrollment` module docs promise the
cap overflow is "a *loud* failure the owner sees" — for pairing, enrollments 2-4 are not surfaced at all.

### S4 — MEDIUM — No revocation; enrolled access outlives the TTL

Two distinct gaps:

- **TTL bounds enrollment, not access.** `ShareRoster::authorize` short-circuits on
  `self.enrolled.contains(&peer)` (`share_enrollment.rs:585`) **before** the expiry check (`:596`). An enrolled peer
  reconnects indefinitely after the 15 minutes elapse. (TTL enforcement is correctly acceptor-side; the initiator
  never reads `ticket.expires_at` — that part is right.)
- **No `pair_cancel`.** `pair_offer` never calls `advertiser.drop_share` (which exists,
  `iroh_advertiser.rs:192`), and `self.rosters` accumulates and is never pruned. An offer cannot be withdrawn before
  TTL, an enrolled device cannot be un-paired, and every container stays advertised for the process lifetime.

### S5 — LOW — `pair_offer` is not idempotent and can strand unrevokable advertisements

`start_share_with_callback` returns `Err("share {id} is already being advertised")`
(`iroh_advertiser.rs:150-154`), so a second `pair_offer` in the same process fails — there is no re-offer after a
TTL expiry. Worse, the mint loop `?`-exits on the first advertise failure: if container *k* fails, containers
`1..k-1` are already advertised with live capability secrets that appear in **no** invite and have no revocation
path.

### S6 — LOW — No cap on `containers.len()`

Measured (scratch test, since deleted): `PairingInvite::decode` **accepts 10,000 containers in 527 ms** — only
`is_empty()` is checked. `pair_with_owner` then builds a 10k-entry ALPN vector and calls `create_endpoint(alpns)`
*before* any per-ticket validation.

The other three abuse inputs behave correctly — garbage base64, valid-base64-non-JSON, and a wrong version all
refuse loudly with specific messages; 8 MiB of junk is refused in 56 ms. **No panic, no hang, no unbounded
allocation** on any of the four. Blast radius is further bounded because each ticket must match a container in the
receiver's own replication set. Self-inflicted (the user pastes the invite), hence LOW.

### S7 — INFO — Test assertions print the full invite

`two_instance_composed_pbt.rs` embeds `\n  offer: {invite}` in its panic messages, so a failing CI run writes N live
`CapabilitySecret`s (for an ephemeral test store) into the log. This is the one path that bypasses
`CapabilitySecret`'s redacted `Debug`. Given this repo's env-leak history, worth noting.

**No production secret leak in the diff.** The new module contains no `tracing`/`println` at all; every `format!`
carries only container ids / `shared_tree_id`, which `share_enrollment`'s threat model already treats as non-secret.
`CapabilitySecret: Debug` is redacted, so `{ticket:?}` and `{invite:?}` are safe. The invite is returned in
`OperationResult::response` (necessary), and both ops use `declared_irreversible(vec![], …)` — an empty
`FieldDelta` set — so `record_history` persists nothing of the invite or its secrets.

## 4. Enrollment leg (e)

`acceptor_enroll` still grants **all-or-nothing**: possession of the capability admits the peer to that container's
full bidirectional sync, with no read/write dimension. **Nothing in this diff widens it** — `share_enrollment.rs`
and `iroh_advertiser.rs` are untouched (not in the 9-file diff).

However, the diff substantially **amplifies the blast radius of that unchanged gap**: previously one capability
admitted one shared subtree; now one invite bundles one capability per container across the entire
`replication_set()`. The D86 decision is correspondingly more load-bearing than when it was deferred.

## 5. Security assumptions this verification rests on

1. iroh's QUIC/TLS binding authenticates `conn.remote_id()` — peer pinning is only as strong as that.
2. `blake3::Hash: PartialEq` is constant-time (as `share_enrollment`'s docs assert); I did not verify it.
3. The invite reaches the second device over a trusted out-of-band channel. There is no SAS ceremony on this path,
   so the invite is a plaintext bearer credential for its TTL — S2/S3 are the consequences.
4. S1 is read-confirmed, not run-confirmed.

## Needs human security review

- **S1** before landing — it is a silent data-loss path through the guard that exists to prevent it, and it is untested.
- **S2** — whether `pair_offer` should refuse `read` until D86, rather than returning success on an unenforceable grant.
- **S3/S4** — whether shipping without revocation, without paired-device visibility, and with a 4-peer replayable
  invite is acceptable for the first production pairing.

---

# Rev 2 — re-verification

**Verdict: CONFIRMED with one residual.** S2–S7 are closed. S4's mechanism is
built as described (and D88's blast radius is now measured). **S1 is only
partly closed**: the journals hole is fixed, but the layout closure now exempts
user content under `block:root-layout` — probe evidence below.

## Gates (once, `-j6`, `RUSTC_WRAPPER=`, `--test-threads 4`)

| Gate | Result |
|---|---|
| `nextest -p holon-loro -p holon-sharing` | **399 run, 399 passed**, 3 skipped (= lane's 406 minus arch's 7) |
| `two_instance_composed_pbt` ×1 | **18 run, 18 passed** (1 slow), 7 skipped |

All eight named Rev-2 tests present and green (`secverify-r2-gate-01.log`):
`…holds_a_journal_note`, `a_read_only_pair_offer_is_refused_until_the_wire_can_enforce_it`,
`an_invite_enrolls_exactly_one_device`, `an_enrolled_peer_is_refused_after_the_capability_expires`,
`pairing_after_pair_cancel_is_refused` (35.8 s), `a_second_pair_offer_while_one_is_live_is_refused`,
`an_invite_with_too_many_containers_is_refused`, `the_invite_fingerprint_never_quotes_the_invite`.

## Per-finding

| # | Verdict | Evidence |
|---|---|---|
| S1 | **PARTLY CLOSED** | journal note refuses; page under `block:root-layout` does NOT — see below |
| S2 | CLOSED | `read` offer refused, refusal contains "D86" |
| S3 | CLOSED | `INVITE_MAX_PEERS = 1`; second device refused |
| S4a | CLOSED (mechanism verified) | expiry moved before the enrolled short-circuit (`share_enrollment.rs:587`) |
| S4b | CLOSED | `device.pair_cancel` present; post-cancel pairing refused |
| S5 | CLOSED | `OfferAlreadyLive` names `device.pair_cancel`; partial advertise now un-advertises |
| S6 | CLOSED | `MAX_INVITE_CONTAINERS = 64` |
| S7 | CLOSED | `invite_fingerprint` (length + truncated blake3); rev-1 leaked invite redacted in place |

### S1 probes (production driver `apply_create_under_focus`)

- **Note under a day block** → refuses, naming the note. PASS.
- **Empty day block** → still app-seeded. Proven by the whole suite: `receiver_day_block`
  asserts a fresh receiver holds exactly one minted day block, and all 18 two_instance
  tests pass, so an empty day block does not trip the guard.
- **Page under `block:root-layout`** → **PAIRING SUCCEEDED.** Scratch probe
  `secverify_scratch_page_block_under_root_layout_is_user_content` failed with
  "pairing SUCCEEDED … the layout closure swallowed user content"
  (`lane-logs/secverify-r2-probe-02.log:179`).

`LAYOUT_ROOT = block:__default__` and its closure is unbounded in depth, so anything
under the layout root — `block:root-layout` included — is classified app-seeded. This is
the rev-1 journals defect relocated to the layout leg.

**Bounding it:** the default seeded receiver (`receiver-root.org`) is still correctly
refused (`secverify_scratch_default_seeded_receiver_is_refused` PASSED), so ordinary
user pages do **not** land under the layout root. The reachable path is content created
while focus is on the layout root, which my probe drove through the production create
driver. Narrower than rev-1's journals hole, but the same class, and untested.
Severity **MEDIUM**.

### S4 / D88 — measured blast radius on `share_subtree`

`ShareRoster::authorize` is shared with subtree sharing, and `loro_share_backend.rs:1674`
is its only other roster mint, with `expires_at = now + DEFAULT_ENROLLMENT_WINDOW_SECS`
(30 days). Scratch measurement on an **already-accepted** share (`secverify-r2-d88-01.log`):

```
D88 day 29: still authorizes
D88 day 31: REFUSED -> share capability expired (now=3678400s > expires_at=3592000s); enrollment refused
```

Blast radius: **every accepted subtree share stops authorizing 30 days after mint**,
already-enrolled peers included — previously it synced indefinitely. `authorize_owner_signed`
is untouched and still has no expiry check, so owner-signed self-devices are unaffected.
Stated as measured; no remedy proposed — this is D88's to rule.

## Teeth (cp-aside inversion → RED → restore)

| Inversion | Observed |
|---|---|
| S1: re-seed the closure from `block:journals` | **RED** — `…holds_a_journal_note` fails: "pairing a receiver that holds a note under its journal day block `block:368857d2-…` SUCCEEDED" |
| S4: enrolled short-circuit back before the expiry check | **RED** — `an_enrolled_peer_is_refused_after_the_capability_expires` fails |

Tree restored byte-identical: sha256 of `two_instance_composed_pbt.rs`,
`device_pairing_op.rs`, `share_enrollment.rs` all match the pre-edit backups; `diff --stat`
unchanged (11 files, 1441 insertions). `cp` only — no `jj restore`. All scratch tests removed.

## Needs human review

- **S1 residual** — content under `block:root-layout` is exempt from `ReceiverNotEmpty`.
  Same defect class as rev-1, one leg over.
- **D88** — the 30-day cliff on accepted subtree shares is a real behavior change,
  measured above.

---

# Rev 3 — delta re-verification

**Verdict: CONFIRMED.** Both Rev-2 residuals are closed. No new finding.

## Gates (once, `-j6`, `RUSTC_WRAPPER=`, `--test-threads 4`)

| Gate | Result |
|---|---|
| `nextest -p holon-loro -p holon-sharing` | **400 run, 400 passed**, 3 skipped |
| `two_instance_composed_pbt` ×1 | **19 run, 19 passed** (1 slow), 7 skipped |

## (1) Enrolled-first restored — D88 cliff gone

Independent repeat of my Rev-2 measurement, same 30-day window
(`secverify-r3-probe-01.log`):

```
R3 enrolled day 29: AUTHORISES
R3 enrolled day 31: AUTHORISES
R3 NEW peer at expiry+1: REFUSED -> share capability expired (now=3592001s > expires_at=3592000s)
```

The Rev-2 30-day cliff on accepted subtree shares is gone; the window now bounds
**enrollment only**, which is what it always documented. Three lane pins present and green,
and the opposite pin (`an_enrolled_peer_is_refused_after_the_capability_expires`) is gone:

- `a_peer_cannot_enroll_after_the_window_closes`
- `an_enrolled_peer_stays_authorised_past_the_window_until_cancelled`
- `an_accepted_subtree_share_still_authorises_its_enrolled_peer_at_day_31` (`loro_share_backend.rs`)

After `pair_cancel`, an enrolled peer is refused — pinned end-to-end by
`pairing_after_pair_cancel_is_refused` (green, 40.5 s). Mechanism note, not a defect:
cancellation is **per-share, not per-peer**. `ShareRoster::authorize` has no way to evict an
enrolled peer; revocation is `drop_share` for pairing and `unshare` for subtree shares. That
is the ruled D88 behavior, but it means there is still no way to un-pair one device while
keeping others.

## (2) Layout family is a closed set — Rev-2 residual closed

My Rev-2 probe, re-run verbatim: a page created under `block:root-layout` through the
production create driver now **refuses**, naming the page
(`secverify_scratch_r3_page_under_layout_root` PASSED).

`block:root-layout` is itself a bundled id (`assets/default/index.org:3`), so the
coordinator's second case — a block whose id is bundled but which has a user child — is the
same probe: the parent is seeded, the child is not, and the child is reported. The
unbounded parent-closure is gone; `seeded` is now the closed set
`{LAYOUT_ROOT} ∪ bundled_layout_ids() ∪ JOURNALS_MACHINERY ∪ empty day blocks`. The lane's
own pin `production_pairing_refuses_a_receiver_that_holds_a_page_under_the_layout_root`
is green.

## (3) `bundled_layout_ids()` is COMPILE-time

`const BUNDLED_LAYOUT: &str = include_str!("../../../assets/default/index.org")`
(`device_pairing_op.rs:74`). The asset is embedded at build time, so the runtime
missing-asset failure mode does not exist — a missing or unreadable asset is a compile
error, and the bad-path scratch probe is moot. No probe written.

Garbling direction is fail-safe: a mangled `:ID:` line drops that id from the closed set,
so the block is reported as user content and pairing **refuses**. The parser cannot
over-seed from a corrupt asset, only under-seed.

## Scope note

Verified only the Rev-3 delta as instructed. The Rev-1/Rev-2 teeth are not re-run; S2, S3,
S5, S6, S7 carry forward from Rev 2 unchanged.

Tree restored byte-identical: sha256 of `two_instance_composed_pbt.rs` and
`share_enrollment.rs` match the pre-edit backups; `diff --stat` unchanged (12 files,
1628 insertions). `cp` only — no `jj restore`. All scratch tests removed.
