# Subtree Sharing (Loro + Iroh)

Two operations let peers collaborate on a subtree of the global Loro tree:

- `share_subtree(id, retention)` on entity `shared_tree` — extract the subtree
  rooted at `id` into its own `LoroDoc`, replace it locally with a mount node,
  advertise the shared doc over Iroh. Returns a base64 ticket.
- `accept_shared_subtree(parent_id, ticket)` on entity `shared_tree` — pull
  the shared doc from the sharing peer and create a mount node under
  `parent_id`.

Both are implemented by `LoroShareBackend` and registered as an
`OperationProvider` by `LoroModule` behind the `iroh-sync` feature flag
(default-on for `holon-loro` and `holon`; off on WASM, where Iroh is
unavailable and docs are local-only).

> ⚠ **This feature is not safe to ship as-is.** A 2026-07 senior review (four
> reviewers, two findings reproduced empirically against loro `1.11.1`) found
> **five blockers**, two of them privacy/data-loss with live prod callers. See
> [Blockers](#blockers-must-fix-before-any-real-share) before enabling sharing
> for real vaults. The design below is otherwise sound; the blockers are
> fixable without redesign.

> ✅ **Status:** B1, B2, B4 and B5 are **fixed**. B3 is **half fixed, and the
> half that works is what makes it dangerous**: text and property writes are
> mount-aware and replicate to the peer, while `create` and `delete` write to
> the global doc and never leave the device, silently. A share therefore looks
> healthy under editing and diverges permanently the moment anyone adds or
> removes a block. See
> [B3](#b3--structural-writes-go-to-the-global-doc-and-never-reach-the-peer)
> and
> `docs/Testing/bugfunnel/entries/2026-09-02-structural-edits-in-a-shared-subtree-never-reach-the-peer.md`.
>
> Sharing is **not** ready for real vaults, and the reason is correctness, not
> authorization: alongside B3, a shared page loses its `Page` tag and becomes
> unreachable in the UI on both peers
> (`docs/Testing/bugfunnel/entries/2026-09-02-sharing-a-page-drops-its-page-tag-and-it-vanishes-from-both-sidebars.md`).
> The blocker sections below are preserved as the review record; each notes its
> resolution inline.

## Ticket format

JSON, wrapped in URL-safe base64 (no padding). Schema v2:

```json
{
  "v": 2,
  "shared_tree_id": "<uuid>",
  "addr": { /* iroh EndpointAddr */ },
  "alpn": "loro-sync/<shared_tree_id>",
  "capability": "<base64 PSK the acceptor proves in the handshake>",
  "expires_at": <unix seconds>
}
```

`capability` carries the pre-shared secret the sync handshake verifies, and
`expires_at` bounds the ticket's life, so a ticket is a **time-limited** bearer
capability rather than an unbounded one.

`v` lets the schema evolve. Decoders reject other versions loudly
(`ticket.rs`).

## Blockers (must fix before any real share)

Severity-ordered. B1 and B2 are **verified empirically** (✅EXP) against loro
`1.11.1`, not inferred from reading. B1 and B3 have **live production
callers today**.

### B1 — `retention="full"` ships the sharer's ENTIRE vault history, and the UI hardcodes it ✅EXP

`frontends/gpui/src/share_ui.rs:300` hardcodes
`params.insert("retention", "full")`. `share_subtree` → `extract_subtree`
(`shared_tree.rs:156-158`) then exports with `ExportMode::Snapshot`.

Fork-and-prune *deletes* the non-shared sibling nodes, but a **full snapshot
retains the complete oplog** — including every op on those deleted nodes. So
the snapshot the accepter fetches contains the plaintext content and edit
history of **every other page/block in the sharer's vault** that existed at
fork time. ✅EXP: a non-shared block's string appears verbatim in the exported
bytes, and `shared.checkout(pre-prune-frontier)` on the accepter's copy
reconstructs the entire pruned personal tree with content.

**Concrete failure:** user shares one grocery-list block; the recipient greps
the snapshot file (or calls `doc.checkout()`) and reads the user's entire
journal.

**Fix:** default the UI to `"none"`; make `"full"` mean "full history *of the
subtree*" by exporting from a clean doc that replays only the kept subtree's
ops (no sibling oplog). Do not ship any `"full"` path that snapshots the forked
global doc.

### B2 — stable peer-id + debounced/unsynced snapshot saves ⇒ counter reuse ⇒ silent permanent divergence ✅EXP

`share_peer_id::stable_peer_id` makes the CRDT peer-id deterministic per
`(device, share)`; `rehydrate_shared_trees` re-applies it over whatever
snapshot is on disk. Saves are **debounced** and `flush_all` runs only on
*graceful* shutdown.

**Sequence:** edit shared doc → sync worker pushes ops `(P, 0..N)` to the peer
→ crash before the debounced save → restart loads a stale snapshot whose VV for
`P` is `< N` → new edits mint `(P, k)` for an already-used counter `k` with
*different* content. ✅EXP: two docs with the same peer-id and divergent ops at
the same counters sync via this exact VV protocol and **both imports return
`Ok` while the docs stay permanently different** — each side's VV claims it
already has the other's ops, so they never converge and nothing errors. Loro
does not detect the collision. The same fires if two processes open one vault
concurrently (the code even anticipates "two rehydration paths got wired up")
or a user re-accepts after wiping local state.

**Fix (pick one, ideally both):** fsync the snapshot before ack'ing any
outbound sync; and mint peer-id from `(device, share, boot-epoch)` or persist a
monotonic counter-floor per share so a reused counter is impossible.

### B3 — structural writes go to the global doc and never reach the peer

⚠ **Half fixed, and the working half hides the broken half.**

Reads follow mount nodes into the shared doc (`loro_backend.rs` mount
traversal). Writes split two ways:

| Write | Doc it lands in | Reaches the peer |
|---|---|---|
| `insert_text` / `delete_text` / `set_field` / `set_state` | shared | yes, seconds |
| `create` / `delete` | global | **no, ever** |

Text and property writes resolve the owning shared doc through the mount
registry (`shared_trees`, `loro_block_operations.rs:67,99`) and replicate
correctly in both directions.

`create` never consults that registry. `loro_block_operations.rs:747-748`
pins the global doc outright:

```rust
// All blocks live in the single global tree
let doc_id = String::new();
```

That comment does not hold once a subtree is shared. `parent_id` is in hand at
that point and is exactly the key that identifies the owning shared doc.
`find_doc_for_block` (`loro_block_operations.rs:106-109`) discards its id
argument and always returns the global backend, so deletes take the same wrong
route.

The consequence is silent permanent divergence: a block added on one device
stays on that device, a block deleted on one device stays alive on the other,
and neither app logs anything. Measured on two live peers, macOS desktop and
Android, in `docs/Testing/bugfunnel/entries/2026-09-02-structural-edits-in-a-shared-subtree-never-reach-the-peer.md`
— including the on-disk proof that a created node sits in `holon_tree.loro`
and is absent from `shares/<id>.loro`.

**Fix:** route `create` by its `parent_id` and the structural ops by their
target id through the same mount registry the text writes already use, so one
resolution step serves every operation. Add a cross-peer convergence invariant
first: a single-instance test cannot see this, because the instance reads back
exactly what it wrote.

✅ **Resolved.** Content writes were routed first; **structural** writes
(create, delete, move) followed on 2026-09-02, after a two-instance dogfood
found them still landing in the global doc. The remaining hole was the MOUNT as
a parent: after a share the page the UI navigates to *is* the mount, so every
create a user drove on a shared page carried the mount's id. `ParentRoute` /
`route_through_mount` in `loro_backend.rs` now resolve a mount parent to its
share's doc with the shared root as the effective parent, and `list_children`
answers for the mount the same way; a mount whose share doc is not loaded is a
loud `Err` naming block and share, never a global write. Covered by
`create_under_mount_node_lands_in_shared_doc`,
`list_children_of_the_mount_lists_the_shared_roots_children`, and the P-STRUCT
oracle in the subtree-share PBT.

⚠ **A structural edit merged against a concurrent one still breaks** — decision
D70. On a shallow share, a structural write panics the pinned loro fork's tree
diff (`tree_state.rs:1198`, `is_node_deleted(target).unwrap()` on a node the
receiving state never saw) as soon as it merges with ANY op the other peer has
not synced yet — the other op does not have to be structural, one peer TYPING is
enough, and it panics in either order. A fully synced text edit followed by a
create survives, so it is the concurrency and not the ops themselves. Shares are
always shallow now that `retention = "full"` is refused, so this is reachable
through ordinary use. Pinned by the `#[ignore]`d
`structure_merged_against_a_concurrent_op_panics_the_shallow_share_engine`, and
the reason the PBT keeps structural writes uncontended.

### B4 — remote-triggered panic + oversized-frame deadlock in the wire framing

`iroh_sync_adapter.rs:86`: `assert!(len <= MAX_MSG_SIZE, ...)`. Any peer can
send a 4-byte length `> 10 MiB` and **panic the accept task** — a remote crash
primitive in an `assert!`, not a `Result`. And `write_framed` never checks
`MAX_MSG_SIZE` while the fallback path deliberately sends **full snapshots**;
with B1's whole-vault snapshot easily exceeding 10 MiB, the sender writes a
frame the receiver is guaranteed to reject → that share can never sync again
(panic on one side, hang in `drain_until_eof` on the other). `(data.len() as
u32)` also truncates silently ≥ 4 GiB.

**Fix:** return a loud `Err` on oversize (both read and write sides); raise or
negotiate the cap; and add the acceptor timeout below (B5-adjacent).

### B5 — no authorization on the accept path; no acceptor timeouts

The incremental acceptor (`sync_doc_handle_connection`) checks **no** peer
allowlist, **no** pre-shared secret, and — unlike the legacy `accept_sync` —
does not even re-verify the ALPN inside the handler. QUIC + iroh node-key TLS
authenticate the *channel* (bytes encrypted, node-id bound) but there is zero
*authorization*: any peer that knows the `shared_tree_id` (it is in the ticket
→ the ALPN) and can reach a live `EndpointAddr` is a full read/write sync peer.
And there are **no timeouts** on the acceptor — the initiator is wrapped in
`CONNECT_TIMEOUT` at its call sites, but every `read_framed` /
`drain_until_eof` / `conn.closed().await` in the accept-loop is an unbounded
wait (trivial slowloris: send a 4-byte length, then stall).

**Resolution:** fixed. The ticket carries a `capability` pre-shared secret that
the handshake verifies, and an `expires_at` that bounds its life (see
[Ticket format](#ticket-format)); the acceptor re-checks the ALPN inside the
handler and every acceptor await is timeout-bounded.

## Threat model

**A ticket is a bearer capability.** Anyone who obtains it can read and write
the shared subtree until the share is dropped (see B5 — there is no authz layer
inside the sync handler). Because relay and discovery are **disabled**
(`RelayMode::Disabled`), an attacker also needs a routable socket address for
the sharing device — the ticket's `addr`, or one leaked via `remember_peer`
into the `.peers.json` sidecar. This raises the bar in practice but is not a
security boundary: LAN addresses are observable and are persisted to disk.

Phase 1 assumes the ticket travels over a trusted channel (iMessage, Signal,
in-person QR scan). UIs that generate a ticket **must** surface a warning that
the ticket should not be posted publicly — **and**, until B1 is fixed, that the
recipient can read the sharer's entire vault history.

### Integrity: SQL projection id collision ✅FIXED

An accepted shared doc is projected into the recipient's SQL `block` table
(`project_descendants_to_sql` + `spawn_projection_worker`), keyed by the
**block ids chosen by the remote sharer**. Shared and locally-authored content
share one id space in one table, distinguished only by a `shared-tree-id`
property. Initial projection uses `create` (INSERT OR IGNORE), so a colliding
id is dropped on accept — but the **ongoing** worker emits `update`/`delete`
via `diff_snapshots_to_ops`. A sharer who knows/guesses a recipient id (e.g. a
well-known seed like `block:journals`) could later mutate a node with that id in
the shared doc and the diff would `update`/`delete` the recipient's own row.
Ids are random UUIDs, so accidental collision is negligible; the risk was a
*deliberate* one.

**Fix (this PR):** rather than the recommended id-namespacing (which would
ripple into the B3 write-routing/rendering that keys off the raw stable id),
both projection paths now enforce an **ownership guard** via
`first_local_collision`. The recipient's global Loro tree is the authority for
local block identity (SQL is projected from it), and a shared subtree's
descendants are pruned from the global tree on share — so under honest
operation no projected id is alive in the global tree. Any projected op whose
id **is** alive in the global tree is therefore a shadow attempt: the initial
projection **rejects the whole accept** loudly, and the ongoing worker
**refuses the op**, emits `ShareDegraded::ForeignIdCollision` (red banner), and
freezes the projection watermark so no clobbering write reaches SQL. Covered by
`projection_worker_refuses_local_id_collision`. Residual (deferred): collisions
between two *different* shared docs' ids are not yet guarded (both are foreign);
only shared-vs-local shadowing is closed here.

## Peer-id derivation

Derived from `(device_secret.public(), shared_tree_id)` via
`share_peer_id::stable_peer_id`; deterministic so a device rejoining the same
share keeps CRDT lineage stable. The device secret is persisted atomically to
`<storage_dir>/device.key` (`device_key_store.rs`).

> ⚠ **`stable_peer_id` uses `std`'s `DefaultHasher`, whose algorithm/seed are
> explicitly not guaranteed stable across Rust std versions.** The whole module
> stakes correctness on "same device+share → same id forever"; a toolchain bump
> can silently shift a device's id after restart → it rejoins its own share as
> a stranger, with the divergence risk of B2. `DefaultHasher` also gives no
> uniformity guarantee for the 64-bit space, and a collision is silent
> unrecoverable divergence (B2). **Replace with an explicit stable hash
> (SipHash with fixed keys, or blake3) plus a persisted per-install salt.**

## Mount nodes

A mount node is an ordinary tree node carrying three metadata keys:
`mount_kind="shared_tree"`, `shared_tree_id`, and `shared_root`. `is_mount_node`
trusts the `mount_kind` string alone; `read_mount_info` parses `shared_root`
with no existence or ownership check. A synced-in remote can therefore stamp
these keys onto any node and it becomes a "mount" pointing at an arbitrary
`shared_tree_id`. It cannot re-parent the global root (a mount is a leaf
reference), but any code that *follows* mounts will chase a forged id. Treat
mount metadata as untrusted on the read side. Malformed `shared_root` currently
**vanishes silently** (`.ok()?` / `_ => None`, `shared_tree.rs:407-422,490-495`)
instead of surfacing corruption — against the repo's "fail loud" rule.

**Sibling position is not preserved.** `commit_share_prune` deletes the subtree
and `create_mount_node` appends a fresh node at the end of the parent's child
list, so the mounted content jumps to the bottom of its siblings. Preserve the
original fractional index.

## End-to-end flow

```text
A                                   B
|-- share_subtree(block_X) ---\    |
|     fork + extract subtree  |    |
|     save shared snapshot    |    |   ⚠ not fsync'd before ack (B2)
|     prune source + mount    |    |   ⚠ no lock vs concurrent edits (see below)
|     save_all global doc     |    |
|     register + workers      |    |   ⚠ sharer never projects subtree→SQL (MAJOR)
|     start IrohAdvertiser    |    |
\---> returns ticket          |    |   ⚠ later failures don't roll back the prune (MAJOR)
                              v    |
                             (out-of-band channel — DM, QR, chat)
                              |    |
                              |    v
                              |    accept_shared_subtree(parent_Y, ticket)
                              |      decode ticket
                              |      start advertiser (long-lived endpoint)
                              |      sync_doc_initiate — pull state (timeout-bounded)
                              |      save shared snapshot
                              |      create mount node under parent_Y
                              |      project mount + descendants into SQL
                              |      register + attach save/sync/projection workers
                              |    <-- returns mount_block_id
```

`share_subtree` sequences fork → snapshot-save → prune and claims to do so
"under the same global-doc write lock" — but `collab.doc()` returns a bare
`Arc<LoroDoc>` clone, so nothing serializes the normal write path against the
extract→prune window (**TOCTOU**: an edit landing between fork and prune is
pruned away and never made it into the shared doc). After the exchange, both
sides advertise on `loro-sync/<shared_tree_id>` and either can dial; the
per-share `sync_worker` fires `sync_with_peers` on local commits (debounced).

**Payloads are plaintext.** No encryption is implemented on the shared doc or
the sync channel beyond whatever iroh's transport provides; do not treat a
share as confidential.

**`HistoryRetention::None` shares cannot merge back.** A shallow snapshot
(`HistoryRetention::None`) breaks CRDT lineage — collaborative edits on that
share do not merge back into the personal tree on unmount. This is not a
silent drop: reintegration now fails loudly (see
`none_retention_reintegration_fails_loudly` in
`crates/holon-loro/src/shared_tree.rs`) rather than quietly discarding the
divergent edits.

## Lifecycle & robustness gaps (MAJOR)

- **Teardown exists.** `unshare` and `gc_orphans` are both registered
  operations on entity `tree`, alongside `share_subtree` and
  `accept_shared_subtree`; `list_operations` on that entity returns all four.
  `unshare` takes the share's `mount_block_id` and tears down in a
  resurrection-safe order — per-share workers first, so no worker can re-write
  the snapshot after it is deleted, then the advertiser endpoint, then the
  shared-doc registration, then the mount node. `gc_orphans` deletes
  `shares/<id>.loro` and its `.peers.json` sidecar for shares with no
  surviving mount node, and returns the deleted ids; it runs only when invoked,
  and any confirmation gate belongs to the UI.
- **Destructive-then-`Err` paths.** After the prune commits, failures in
  `save_all` / `start_advertising_stable` / `Ticket::encode` all return `Err`
  with the subtree already destructively replaced by a mount — the caller is
  told the share failed. Accept mirrors this: it returns `Err` after the mount
  is durable if SQL projection fails, leaving a mount with no registered doc
  and no workers until restart rehydration.
- **Sharer side never projects the subtree into SQL.** `accept` and `rehydrate`
  call `project_*_to_sql`; `share_subtree` calls neither, and the projection
  worker's watermark starts at spawn-time frontiers, so pre-existing content is
  never diffed. Meanwhile the mount commit's outbound diff emits deletes for
  every pruned descendant → on the sharing peer the UI loses the shared subtree
  until the next restart repairs it.
- **Projection failure is invisible.** When `execute_batch_with_origin` fails,
  the worker only `tracing::error!`s — no `DegradedSignalBus` emit (unlike the
  save worker). If the user stops editing, Loro and SQL diverge silently with
  no banner — the exact failure class "fail loud, never fake" targets.
- **`sync_with_peers` swallows every per-peer failure** (`warn!` then
  `Ok(synced)`); "all peers unreachable forever" is a debug non-event.
- **Torn-snapshot race.** `flush_all` calls `snapshot_store.save` directly
  while the debounced save worker for the same id can fire concurrently; both
  write `<id>.loro.tmp` via separate truncating fds then rename — one can
  promote a partial file, which the next startup quarantines as corrupt.
- **Blocking fsync on the tokio runtime.** Snapshot/peer/port writes call
  `write_all` + `sync_all` directly (no `spawn_blocking`), including inside
  `share_subtree`'s critical section — an fsync stall freezes an executor
  thread.
- **`spawn_projection_worker` `expect`s the mount URI** parsed from arbitrary
  Loro `STABLE_ID` metadata — one malformed id panics the backend during
  startup rehydration instead of degraded-skipping that one share.

## Concurrency

- **Simultaneous-dial race.** Both peers advertise and both auto-resync on
  commit (`SYNC_DEBOUNCE = 500ms`); nothing assigns initiator vs acceptor role
  per share. If A and B dial each other in the same window you get two
  concurrent handshakes mutating the same `Arc<LoroDoc>` from two tasks; the
  protocol's EOF/close reasoning assumes exactly one initiator. Add a
  tiebreaker (e.g. lower peer-id initiates).
- **No per-doc serialization in the acceptor.** The `RwLock` guards the doc
  *map*, not each doc; two concurrent inbound syncs on one doc are not
  serialized. Loro merge is commutative so state converges, but a delta can be
  computed against a stale `peer_vv` mid-flight. Add a per-doc mutex around
  read-VV → export → import.
- The lock story is otherwise clean: guards are consistently dropped before
  awaits/IO, so there is no deadlock — the problems above are atomicity and
  visibility, not lock ordering.

## Files

- `crates/holon-loro/src/ticket.rs` — ticket encode/decode.
- `crates/holon-loro/src/iroh_advertiser.rs` — persistent accepter pool.
- `crates/holon-loro/src/iroh_sync_adapter.rs` — VV-based wire protocol,
  framing, endpoints.
- `crates/holon-loro/src/share_peer_id.rs` — stable peer-id derivation.
- `crates/holon-loro/src/device_key_store.rs` — persistent device secret.
- `crates/holon-loro/src/shared_tree.rs` — fork-and-prune + mount nodes.
- `crates/holon-loro/src/loro_share_backend.rs` — `SubtreeShareOperations`
  trait + impl + `OperationProvider` wiring + SQL projection workers.
- `crates/holon-loro/src/multi_peer.rs` — **proptest reference model only**
  (`DirectSync`/`PeerState`/`GroupState`); no network layer. Currently
  `pub mod` with no `#[cfg(test)]`/feature gate, so it drags `proptest` into
  prod builds of `holon-loro` (see the crate-slicing audit). `SyncBackend` is
  also duplicated verbatim between this file and `iroh_sync_adapter.rs`.
- `frontends/gpui/src/share_ui.rs` — share UI (hardcodes `retention="full"`,
  see B1).
- `crates/holon/src/sync/loro_module.rs` — DI wiring gated on `iroh-sync`.

## Known gaps (lower severity, previously tracked)

1. **Nested shares only shallowly rejected.** `share_subtree` rejects sharing a
   node that *is* a mount but not a subtree that *contains* one deeper down;
   the inner mount's metadata is copied into the shared doc but its
   `shared_tree_id` isn't in the accepter's store, so that content silently
   vanishes on the accepter. Accepting *into* an already-shared subtree is also
   unguarded. Forbid mounts anywhere in the extracted set, or handle them.
2. **No cross-doc cycle guard.** Mounts reference shared docs by id through an
   external store; nothing prevents A mounting B while B mounts A → unbounded
   traversal.
3. **`unmount` reintegration is a latent vault-wiper** (test-only callers
   today). Reintegration `import`s the shared full snapshot, which per B1
   carries the fork's prune ops (delete root + every sibling) authored by the
   fork's fresh peer with later lamports. ✅EXP: after
   `unmount(_, Some(shared))` both the root and a never-shared sibling are dead
   in the source doc. The existing test only checks the shared block survived.
   Fix before wiring `unmount` to any UI.
4. **`HistoryRetention::Since(Frontiers)`** — not selectable from the string
   param yet; ticket schema extension needed.
5. **Concurrent share of the same block from two devices** — last-writer-wins
   on mount replacement; no property test.
6. **`OperationDispatcher` silently picks the first provider** on an
   `(entity, op_name)` collision. The fresh `shared_tree` entity sidesteps it
   here, but the dispatcher behaviour should become an error.
7. **Shares survive restart via `rehydrate_shared_trees`** (mount nodes are
   authoritative; snapshots + peer sidecars reload). The remaining gap: a
   changed advertiser port partitions peers until this device dials out first.
   Sidecar hygiene is also incomplete (`.port.tmp` never swept; `.port` never
   deleted by `gc_orphans`).

## Validation status

Verified against jj workspace `default` @ change `zxpozxzn` / commit
`647c54ff`. All code claims were read directly in that checkout. B1 (history
leak / time-travel) and B2 (duplicate-peer-id silent non-convergence), plus the
`unmount` data loss (gap 3), were **reproduced out-of-tree against loro
`1.11.1`** (the pinned version) — marked ✅EXP. No other workspace matched a
different state; findings are current, not plan drift.

Property tests worth adding (each proves a blocker's fix):
- a `"full"`-retention shared doc, replayed, contains **no** content from
  sibling subtrees (B1);
- a crash between outbound sync and snapshot save does not mint a reused peer
  counter (B2);
- two peers holding one share converge after **every** operation kind, `create`
  and `delete` included, comparing the subtree by `(child id, content,
  properties)` — each peer mints its own mount-block id, so comparing
  `parent_id` reports a false divergence (B3);
- a peer sending a 4-byte length then stalling does not hang the acceptor past
  a timeout, and an oversize length returns `Err` (not panic) (B4/B5);
- `unmount` with reintegration leaves all kept (non-shared) nodes alive
  (gap 3).
