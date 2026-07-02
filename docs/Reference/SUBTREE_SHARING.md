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

## Ticket format

JSON, wrapped in URL-safe base64 (no padding). Schema v1:

```json
{
  "v": 1,
  "shared_tree_id": "<uuid>",
  "addr": { /* iroh EndpointAddr */ },
  "alpn": "loro-sync/<shared_tree_id>"
}
```

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

### B3 — edits to shared content route to the global doc and are silently lost

Reads follow mount nodes into the shared doc (`loro_backend.rs` mount
traversal), but **every write** (`update_block_text`, `update_block`,
`move_block`, …) does `self.collab_doc.with_write(...)` on the **global** doc
only — no write path targets a shared doc. Compounding it, `share_subtree`
prunes the subtree directly on the doc without invalidating `LoroBackend`'s
`id_cache`, so on the sharer a post-share edit resolves the stale cached
`TreeID`, writes into the *deleted* global node, and is never rendered (reads
come from the shared doc) nor synced. On the accepter the same edit returns
`BlockNotFound`.

**Fix:** either sharing is read-only by design — then reject writes to
shared/mounted content loudly on both sides — or add mount-aware write routing
plus cache invalidation on prune. Ship neither silently.

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

**Fix (Phase 2):** ticket-embedded pre-shared secret verified in the handshake;
per-share peer-id allowlist checked inside `sync_doc_handle_connection`; a
timeout around every acceptor await.

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

### Integrity: SQL projection has no id namespacing

An accepted shared doc is projected into the recipient's SQL `block` table
(`project_descendants_to_sql` + `spawn_projection_worker`), keyed by the
**block ids chosen by the remote sharer**. Shared and locally-authored content
share one id space in one table, distinguished only by a `shared-tree-id`
property. Initial projection uses `create` (INSERT OR IGNORE), so a colliding
id is dropped on accept — but the **ongoing** worker emits `update`/`delete`
via `diff_snapshots_to_ops`. A sharer who knows/guesses a recipient id (e.g. a
well-known seed like `block:journals`) can later mutate a node with that id in
the shared doc and the diff will `update`/`delete` the recipient's own row.
Ids are random UUIDs, so accidental collision is negligible; the risk is a
*deliberate* one. **Recommended:** namespace projected ids by
`shared_tree_id`, or reject any projected op whose target already exists as a
non-shared block.

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

## Lifecycle & robustness gaps (MAJOR)

- **No unshare / `drop_share` path.** The `save_workers` doc-comment promises
  "dropped when `unregister` is called" but no such method exists; nothing
  calls `IrohAdvertiser::drop_share`. Manager registration, three workers, and
  the advertiser endpoint leak per share for the process lifetime. If the user
  deletes the mount block, `gc_orphans` deletes `<id>.loro` but the still-
  attached save worker re-writes it on the next commit, un-deleting the share.
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
- a write to shared/mounted content either syncs or is rejected loudly, never
  silently dropped (B3);
- a peer sending a 4-byte length then stalling does not hang the acceptor past
  a timeout, and an oversize length returns `Err` (not panic) (B4/B5);
- `unmount` with reintegration leaves all kept (non-shared) nodes alive
  (gap 3).
