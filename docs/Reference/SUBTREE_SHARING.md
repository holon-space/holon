# Subtree Sharing (Loro + Iroh)

Two operations let peers collaborate on a subtree of the global Loro tree:

- `share_subtree(id, retention)` on entity `shared_tree` — extract the subtree
  rooted at `id` into its own `LoroDoc`, replace it locally with a mount node,
  advertise the shared doc over Iroh. Returns a base64 ticket.
- `accept_shared_subtree(parent_id, ticket)` on entity `shared_tree` — pull
  the shared doc from the sharing peer and create a mount node under
  `parent_id`.

Both are implemented by `LoroShareBackend` and registered as an
`OperationProvider` by `LoroModule` behind the `iroh-sync` feature flag.

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

`v` lets the schema evolve. Decoders reject other versions loudly.

## Threat model

**A ticket is a bearer capability.** Anyone who obtains it can read and write
the shared subtree until the share is dropped. There is no authn/authz layer
inside iroh — peer identity is the only gate, and the initial handshake does
not verify "who you are" beyond a cryptographic node id.

Phase 1 assumes the ticket travels over a trusted channel (iMessage, Signal,
in-person QR scan). UIs that generate a ticket **must** surface a warning
that the ticket should not be posted publicly.

Phase 2 (not yet implemented) will add:
- a ticket-embedded pre-shared secret verified during the sync handshake;
- a per-share peer-id allowlist;
- a `drop_share` operation that closes the endpoint and removes the
  registration.

## Peer-id derivation

Each `LoroDoc` needs a unique `peer_id` per participant. We derive it from
`(device_secret.public(), shared_tree_id)` via `share_peer_id::stable_peer_id`.
This is deterministic — the same device rejoining the same share always uses
the same peer-id, which keeps CRDT lineage stable across restarts.

## Files

- `crates/holon/src/sync/ticket.rs` — ticket encode/decode.
- `crates/holon/src/sync/iroh_advertiser.rs` — persistent accepter pool.
- `crates/holon/src/sync/share_peer_id.rs` — stable peer-id derivation.
- `crates/holon/src/sync/shared_tree.rs` — fork-and-prune + mount nodes
  (pre-existing).
- `crates/holon/src/sync/loro_share_backend.rs` — `SubtreeShareOperations`
  trait + impl + `OperationProvider` wiring.
- `crates/holon/src/sync/loro_module.rs` — DI wiring gated on `iroh-sync`.

## End-to-end flow

```text
A                                   B
|-- share_subtree(block_X) ---\    |
|     extract subtree         |    |
|     replace with mount      |    |
|     register in manager     |    |
|     start IrohAdvertiser    |    |
\---> returns ticket          |    |
                              v    |
                             (out-of-band channel — DM, QR, chat)
                              |    |
                              |    v
                              |    accept_shared_subtree(parent_Y, ticket)
                              |      decode ticket
                              |      sync_doc_initiate — pull state
                              |      register in manager
                              |      start IrohAdvertiser (bidirectional)
                              |      create mount node under parent_Y
                              |    <-- returns mount_block_id
```

After the initial exchange, both sides are running accepters on the same
ALPN (`loro-sync/<shared_tree_id>`), so either can dial the other to push
subsequent updates. (Advertiser-to-advertiser push is not yet implemented;
Phase 2 will add either a periodic exchange or a subscribe-and-push path.)

## Known gaps (Phase 2)

1. `HistoryRetention::Since(Frontiers)` — not selectable from the string
   parameter yet; ticket schema extension needed.
2. Nested shares (sharing a subtree that already contains a mount) — rejected
   in Phase 1.
3. Concurrent share of the same block from two devices — last-writer-wins on
   mount replacement; not yet covered by a property test.
4. Shares do not survive a restart: advertisers are ephemeral. A persistent
   `shared_tree_id → metadata` table + boot-time re-advertise is planned.
5. `OperationDispatcher` silently picks the first provider on an
   `(entity, op_name)` collision. Using the fresh `shared_tree` entity
   sidesteps the risk here, but the dispatcher behaviour itself should become
   an error — tracked separately.
