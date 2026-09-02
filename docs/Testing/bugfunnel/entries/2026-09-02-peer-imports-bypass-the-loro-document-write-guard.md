---
id: 2026-09-02-peer-imports-bypass-the-loro-document-write-guard
date: 2026-09-02
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
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

Open. Two candidate fixes, and the choice belongs to the sharing track:

- **(a) Route the accept loop's import through `LoroDocument`.** The advertiser
  would have to carry the wrapper rather than the raw `Arc<LoroDoc>`, which
  cuts against `replicate_all`'s blind-relay guardrail ([SR]) that the
  transport treats the payload as opaque. Passing the wrapper does not
  actually break that — the transport still never reads the doc — but it does
  widen what the advertiser holds.
- **(b) Narrow the guard's stated contract** and say in
  `loro_document.rs:174-190` that peer imports are deliberately outside it,
  with the reason. Only honest if the interleaving is genuinely safe, which
  nobody has established.

(a) is the fix if the contract as written is meant. Whichever is chosen, the
ORACLE half should land with it, or the next transport will bypass the guard
the same way.
