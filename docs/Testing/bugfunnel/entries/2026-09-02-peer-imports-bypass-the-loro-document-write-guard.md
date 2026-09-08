---
id: 2026-09-02-peer-imports-bypass-the-loro-document-write-guard
date: 2026-09-02
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  A peer import arriving over the production iroh transport writes into the
  global Loro doc without taking the doc-boundary write guard and without an
  origin tag, so it can interleave with a local write batch's interior — the
  exact state `LoroDocument::with_write_origin` exists to make unobservable.
---

## Bug

Found by the `pair-inc3` lane on 2026-09-02 while parameterising the
two-instance composed PBT over the real transport (ruling D71.b). Not a runtime
failure — found by reading the two code paths side by side once the same
property ran over both of them.

The two-instance slice imports peer state one way in the test model and a
different way in production, and only the model's way is guarded.

| Leg | Import call | Doc write guard | Origin tag |
|---|---|---|---|
| relay model | `LoroDocument::apply_update_with_origin` | held | `sync_import` |
| production (iroh) | raw `LoroDoc::import` | **none** | loro default |

## Root cause

`LoroDocument` exists to serialise access to the raw `Arc<LoroDoc>` behind a
per-document lock. `crates/holon-loro/src/loro_document.rs:149-155`:

```rust
pub fn apply_update_with_origin(&self, origin: &str, update: &[u8]) -> Result<()> {
    self.lock.write(&self.doc_id, || {
        self.doc.import_with(update, origin)?;
```

The guard's own contract, at `loro_document.rs:174-190`, states what it
protects: `with_write_origin` holds the lock across the whole closure *and* the
trailing `commit()`, "so no reader, exporter or saver can observe the batch
interior". An import that does not take the same lock is exactly such an
observer, and worse — it is a *writer* that can land between a batch's ops and
its commit.

The relay leg honours it. `crates/holon-sharing/src/sync.rs:233` imports an
admitted envelope through `apply_update_with_origin("sync_import", …)`.

The production leg does not. Both sides of the version-vector exchange import
the peer delta with a bare `doc.import`, holding no lock:

- initiator: `crates/holon-loro/src/iroh_sync_adapter.rs:312`
- acceptor: `crates/holon-loro/src/iroh_sync_adapter.rs:426`, reached from
  `sync_doc_handle_connection` (`:390`), which is what
  `ContainerRegistry::replicate_all` (`container_registry.rs:235`) wires the
  accept loop to.

The doc handed in is the live global document — `replicate_all` passes
`container.doc.doc()`, escaping the `LoroDocument` wrapper by design so the
transport can treat the payload as opaque. Escaping the wrapper also escapes its
lock.

Two consequences, in order of severity:

1. **No mutual exclusion with local writes.** The accept loop runs on its own
   spawned task, so a peer import races any concurrent `with_write` batch on
   the same document. Nothing serialises them.
2. **No origin tag.** Subscribers see loro's default origin rather than
   `sync_import`, so a subscriber cannot tell a peer import from a local write.
   Nothing keys on the origin today, which is why this has stayed invisible;
   it is a trap for the next subscriber that does.

This is a product defect, not a test artifact. The unguarded path is the one
that ships: it is how the paired Mac and phone in the `double-dogfood` lane
exchanged state.

## Missing piece

**ENVIRONMENT (primary).** Until this lane, no composed test ran the production
import path at all. Every two-instance test imported through the guarded relay,
so the unguarded call site had no test-side existence and prod/test parity hid
the difference rather than exposing it. The parity seam this lane added
(`crates/holon-integration-tests/src/pbt/composed/two_instance_transport.rs`)
is what makes the path reachable from a test for the first time.

**ORACLE (secondary).** Even now that the path runs, nothing would flag it. No
invariant asserts that every write into a replicated document was made under
the document's own guard, so a case that interleaved an import with a local
batch would corrupt state silently rather than go red. The natural shape is a
guard-witness on `LoroDocument` (a counter of writes that bypassed the lock,
asserted zero) rather than an attempt to schedule the race.

## Keystone repro

The keystone (`tests/general_e2e_composed_pbt.rs`) cannot reproduce it: it boots
one instance, and there is no peer to import from. The two-instance binary now
*executes* the unguarded path on its iroh leg, on every round, but does not yet
*detect* the hazard — the race is timing-dependent and no assertion covers it.

## Remedy

FIXED by the `sharing-admit` lane (2026-09-08), taking option **(a)** — the
contract in `loro_document.rs` is meant, so the accept loop honours it.

Both iroh legs now import through `holon_loro::peer_import::import_peer_delta`,
which re-wraps the transport's `Arc<LoroDoc>` and calls
`apply_update_with_origin(SYNC_IMPORT_ORIGIN, …)`. The advertiser keeps handing
the transport a raw `Arc` — the blind-relay guardrail [SR] is untouched — because
re-wrapping resolves to the SAME lock: `DocLock::for_doc` keys the registry by
`Arc::as_ptr` (`doc_lock.rs:44-67`), which `LoroDocument::doc`'s own doc comment
already promised and `doc_lock::tests::two_wrappers_over_one_inner_doc_share_one_lock`
already pinned. So the wrapper did not have to travel through the advertiser at
all; only the import call site had to stop escaping.

Both call sites changed: `iroh_sync_adapter.rs` initiator (was `:312`) and
acceptor (was `:426`). The origin literal is now one constant,
`holon_loro::loro_document::SYNC_IMPORT_ORIGIN`, shared with the relay leg
(`holon-sharing/src/sync.rs`), so the two legs cannot drift.

The ORACLE half landed with it, in the shape the entry asked for — an assertion
on the observable rather than an attempt to schedule the race:

- `holon-loro` `iroh_sync_adapter::adapter::tests::both_iroh_legs_tag_a_peer_delta_sync_import`
  runs a real iroh round with an edit on each side and asserts BOTH legs saw a
  `sync_import`-tagged commit. Red-for-the-right-reason before the fix:
  `the initiator leg imported a peer delta under origin(s) [""], none of them
  `sync_import` — the import bypassed `LoroDocument`'s write guard`.
- `holon-loro` `peer_import::tests::a_read_write_peers_delta_lands_tagged_sync_import`
  pins the same property at unit level, without a network.

The deeper guard-witness the entry proposed (a counter of writes that bypassed
the lock) was not needed: `import_peer_delta` is now the ONLY function in
`holon-loro` that writes peer bytes into a doc, and it cannot be called without
an `AdmittedPeer`, so a future transport cannot repeat the bypass without
deleting a type. `archlint`'s `loro_doc_escape` rule already flags any new raw
`.doc()` escape.
