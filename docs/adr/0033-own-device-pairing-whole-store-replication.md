# ADR 0033 — Own-device pairing: whole-store replication with a device-local layout

**Status:** Accepted (D68.b, D71.b, D72.a, D73.a, D75.a ratified by Martin
2026-09-02; D77.b and D78.d ratified 2026-09-03, **implementation in flight**)
**Date:** 2026-09-03
**Deciders:** Martin (decision-inbox rulings D68–D78)
**Relates to:**
[ADR 0028](0028-sharing-policy-overlay.md) — container-scoped sharing and the
capability vocabulary this ADR enforces at the acceptor.
[ADR 0030](0030-birth-atomicity-authority-and-mirror-contract.md) — one
authority store per birth; the re-import path mints through that authority.
`docs/Architecture/Model.md` — invariant 11 (one consolidator per file replica)
and its shared-subtree carve-out.
`docs/Architecture/Replication.md` — capability profiles and the two transports.
`docs/Reference/SUBTREE_SHARING.md` — per-subtree mounts, which pairing refuses
to coexist with.

## Problem

A user runs Holon on a Mac and on a phone and wants the same vault on both.
Holon already has per-subtree sharing: a page is published into its own shared
`LoroDoc` and the peer mounts it. Reusing that for "my own two devices" means
one share per top-level page, a mount per share, and a growing list of things
the user must remember to publish.

Four questions had no settled answer:

1. **What unit replicates** — a subtree per share, or the whole store?
2. **What carries the UI layout**, which is legitimately different per device
   (a phone's panel arrangement is not the Mac's) yet lives in the same store as
   content.
3. **What the production path is**, given that the two-instance property test
   drives an in-memory relay and production drives iroh — a test that exercises
   a different wire than production proves less than it appears to.
4. **What happens to a phone that was already used** before it was paired. Both
   devices run the same boot code, and that code seeds blocks with FIXED ids
   (`block:journals`, the bundled layout under `block:__default__`, a day block
   keyed by date). A Loro tree node is identified by the CRDT as (peer, counter),
   not by a stable id, so two devices that seed independently and then merge hold
   TWO live nodes claiming one stable id. Readers keyed by stable id collapse
   them, so the duplicate stays invisible until a child lands under the wrong
   one. Measured after one whole-vault round: 31 doubled ids — 25 bundled
   layout, 5 journals machinery, 1 today's day block.

## Decision

### 1. Pairing replicates the WHOLE store; the layout stays device-local

Own-device pairing is not a share. It replicates every replicable container in
the store as one unit, and the user names no pages. The UI layout is the
explicit exception: it never leaves the device.

### 2. The layout lives in a second, device-local document

`LoroDocumentStore` holds a second `layout_doc`, persisted as its own
`holon_layout.loro`, always compiled, and **never registered in
`ContainerRegistry`** — which is what makes it unreachable by replication rather
than merely skipped by it. The backend probes layout-then-global on reads and
routes creates by the parent's document. `block:__default__` moves into the
layout document once, at boot.

Registry membership, not a filter, is the enforcement point: a container the
registry does not hold cannot be advertised, so no later replication path can
re-include the layout by forgetting a condition.

### 3. Production replicates through `ContainerRegistry::replicate_all` over iroh

The production pairing path is `ContainerRegistry::replicate_all`
(`crates/holon-loro/src/container_registry.rs:235`) driven over the iroh
advertiser. The in-memory relay stays in the property harness as the *model*
leg, and the two-instance composed property is parameterised over BOTH legs
(`crates/holon-integration-tests/src/pbt/composed/two_instance_transport.rs`) so
the model and production wires are held to one oracle. A property that only ever
ran on the relay would let the wire drift away from what ships.

### 4. Read/write capability is enforced at ONE acceptor boundary

`Capability::Write` (`crates/holon-sharing/src/policy.rs:59`) is checked at the
acceptor (`crates/holon-sharing/src/acceptor.rs`) and nowhere else. A paired
phone is a FULL WRITER: it holds the same store and writes into it.

Disclosed limits of the current writer story: write-back to org files happens
only while the Mac runs, and the phone has no write-back leg of its own. Org
files plus git are the rollback path.

### 5. Pairing refuses while the receiver holds per-subtree mounts

If the receiving device has any per-subtree mount, pairing refuses loudly and
names each mount. Whole-store replication and a mounted subtree disagree about
which document owns a node, and the refusal keeps that disagreement from
reaching the merge.

### 6. Pairing a used phone: archive, bootstrap, re-import

There is no pair-or-solo question at first open, ever. A user may install Holon
on a phone, use it for a week, and pair later without losing anything.

Pairing a non-empty receiver does NOT merge two CRDT histories. Three steps run
on the receiver before it joins replication:

1. **Archive.** The receiver's `holon.loro` moves aside into a timestamped
   archive directory. Nothing is deleted, so the step is reversible.
2. **Bootstrap.** The receiver's document is CREATED by importing the owner's
   snapshot. Creating it by import is what makes a shallow (compacted) owner
   work: importing a shallow snapshot into a non-empty document drops the base.
   After this step every fixed id names exactly the owner's node.
3. **Re-import.** The archived store is walked and the phone's USER content is
   replayed as ordinary new operations through the typed-rows ingest seam — the
   same path org files and external peers use (`TypedRowSet` in
   `crates/holon-core/src/file_format.rs:52`, `DispatchingTypedRowSink` in
   `crates/holon/src/core/typed_row_sink.rs:34`). Fixed-id nodes in the archive
   are not re-created; a fixed-id re-parent table says where their children go.
   A day block whose date already exists on the owner's side has its children
   appended under the owner's day block. User blocks keep their uuids — only
   fixed ids are shared by construction, so uuids cannot collide.

Worked example. A phone used solo for a week holds 6 journal days, 40 blocks and
a page "Ideas"; the owner vault holds 900 pages. Pairing archives the phone
store, brings in the 900 pages, appends the 6 days under the owner's
`block:journals` (2 of them merging into days the owner also wrote), and lands
"Ideas" as a new page with its uuid intact. Links between the phone's own blocks
still resolve, and the shared history never contained a duplicate
`block:journals`.

**The cost, stated:** the phone's pre-pair edit history is flattened to a
snapshot, so undo across the pairing boundary is gone. What is bought: the shared
history never holds a duplicate fixed id, and pairing writes no CRDT DELETE on
the strength of a guess about user intent.

**Boot assertion.** Every fixed id resolves to exactly one live node; a violation
fails loud. That assertion is what keeps the re-parent table honest when a new
fixed id is added and its row is forgotten.

Lazy seeding rides along: `block:journals` and the day block are not seeded until
the user's first write, so a phone installed and paired before any write mints
nothing at all.

### 7. Unkeyed `BlobSig` is deferred; own-device pairing is direct iroh only

`BlobSig` (`crates/holon-sharing/src/acceptor.rs:126`) carries a blake3 digest
with no sender authentication. Own-device pairing ships over direct iroh/QUIC
only, where the QUIC peer identity is the authentication. `BlobSig` becomes keyed
— Ed25519 over the canonical bytes, signer = the chain's terminal grantee —
before any relay-backed or third-party share lands. This is tracked as an open
security item, not a closed one.

## Consequences

- The user pairs devices, not pages. Nothing has to be published for the phone to
  hold the vault.
- The layout document is outside `ContainerRegistry`, so any code that enumerates
  replicable containers gets the right answer without knowing about layouts.
- The two-instance property tests the production wire, so relay-versus-iroh drift
  fails a gate rather than a dogfood session.
- Pairing after solo use is an ingest problem, not a pairing special case: the
  re-import reuses the generic typed-rows seam.
- The pre-pair edit history is not recoverable through undo after pairing; the
  archive directory is the only route back to it.
- The fixed-id re-parent table is a maintenance obligation. Every new fixed id
  needs a row, and the boot assertion is what catches a missing one.

## Superseded

**D74.a ("pairing requires an empty receiver") is superseded by §6.** A non-empty
receiver is archived and re-imported, never refused. The loud refusal survives
only for mounts (§5). No pair-versus-solo gating UI is to be built.

## Alternatives considered

**Merge both CRDT histories, then repair fixed ids deterministically.** Pair as
today, then let both devices compute the same repair from converged state — keep
the node created by the lower peer id, move the other's children under it, delete
the loser. It preserves the phone's full edit history in the shared CRDT.
Rejected: it writes DELETEs into shared history from a rule, the repair must be
idempotent under concurrent edits to both twins, and the duplicate stays in the
history forever as a tombstone.

**Lazy seeding alone.** Shrinks the window to "used before pairing" and does not
close it. Kept as a rider (§6), not as the answer.

**Deterministic op ids for fixed-id seeds** — seed under a reserved peer id with
fixed counters so both devices emit the same Loro op and the CRDT deduplicates
it. Rejected: two documents generating different ops under one peer id is
undefined behaviour in Loro, and any seed drift (a layout change, a version skew)
corrupts the history.

**One share per top-level page.** The per-subtree sharing machinery already
exists, so this is the cheap path. Rejected: it makes the user curate the list of
what their own phone may see, and every page added later is a share they must
remember to create.
