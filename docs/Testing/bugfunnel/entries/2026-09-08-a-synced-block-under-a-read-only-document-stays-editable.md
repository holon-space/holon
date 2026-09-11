---
id: 2026-09-08-a-synced-block-under-a-read-only-document-stays-editable
date: 2026-09-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block a peer's sync import places under a read-only-homed document is exempt
  from the write-tier gate, so it is absent from the membership registry and
  stays fully editable — an edit no writer can ever put into the authoritative
  file.
---

## Bug

Found by the fresh-context verifier of lane `readonly-invariant` (outside any
test), reading `enforce_write_tier`. The gate exempted `OpOrigin::Sync`
wholesale:

    crates/holon/src/api/operation_dispatcher.rs:536
    if resolved_entity_name != "block" || matches!(origin, OpOrigin::Ingest | OpOrigin::Sync)

The registry the gate consults (`ReadOnlyDocuments`) is filled ONLY by the
file-sync controller, from the file's own parse. A block a peer adds under a
read-only document's root therefore appears in no membership, and
`refusal_for_block` answers `None` for it — the block renders with the full
editing affordance set and every edit to it is accepted.

Nothing can ever write that block into the authoritative file: cooklang and
every other `WriteTier::ReadOnly` format refuses write-back by design. So the
store diverges from the disk exactly as it did before the refusal existed, only
reached through pairing instead of through typing.

Not yet reachable in production — no production site dispatches with
`OpOrigin::Sync` today — which is why it was a reading finding rather than a
dogfood one. It becomes reachable the moment pairing meets a vault holding a
`.cook` file.

## Root cause

The exemption was stated as "a peer's already-merged history — refusing it would
break the replica it comes from". That is right about REFUSING and silent about
the second obligation: the membership. The tier of a document is a property of
where the document is homed, not of who wrote to it, so an import that lands
under a read-only root must still leave the store in a state where nothing can
edit what the disk cannot take.

## Missing piece

COVERAGE. The write-tier invariant was pinned for store-origin writes only —
`inv-read-only-home-refuses-writes` drives `AttemptReadOnlyEdit`, which
dispatches as a user. No transition or test dispatched anything under a
read-only root with a non-user origin, so the exemption was never exercised.
The environment was adequate (the keystone's `wide_e2e` fixture holds a real
`.cook` file); the case simply was not generated.

## Fix

The import lands and INHERITS the tier — the choice consistent with how the
gate already treats sync: the merge has already happened in the peer, so
refusing here would only make this store disagree with the replica while the
file stays unwritable either way. `ReadOnlyDocuments::adopt` binds the imported
block to the document that owns its parent, and the dispatcher calls it on every
`OpOrigin::Sync` block write that names a parent. Adopted blocks are held apart
from the file-declared membership so a re-ingest — which re-derives that
membership FROM the file, which never declares them — cannot silently make them
editable again; only the document's own disappearance (`forget`) ends the
binding.

Covered by, in `crates/holon-core/src/write_tier_gate.rs`:

  - `a_block_imported_under_a_read_only_document_earns_the_refusal`
  - `an_imported_block_survives_a_re_ingest_and_ends_with_the_document`

and at the dispatcher by
`crates/holon/src/api/operation_dispatcher.rs`'s
`a_sync_import_under_a_read_only_root_is_adopted_not_left_editable`.

## Residual — two; #1 CLOSED 2026-09-11, #2 still open

**1. The Loro import leg did not reach the adoption at all. CLOSED
(2026-09-11, lane `loro-import-origin`).**

The mechanism recorded here on 2026-09-08 was wrong in a way worth keeping,
because it made the residual sound milder than it was. `LoroShareBackend`'s
`sql_ops` is not the operation dispatcher: DI hands it a bare
`SqlOperationProvider` (`holon_loro_wiring::block_sql_write_provider`,
`crates/holon-loro-wiring/src/loro_module.rs`). So the share projection legs
never entered `execute_operation_with_provenance` at all, and no origin —
`User` or otherwise — was ever assigned to them. The claimed consequence, that
such an import would be REFUSED outright as a user write, was false: nothing
judged it. It landed, fully editable, exactly the state this entry set out to
prevent. Measured, not reasoned: the new test's first assertion (the row
reaches SQL) passed while red.

`OpOrigin::Sync` is still constructed nowhere in production — it cannot be
threaded through a dispatcher that is not on this path. The fix instead puts
the dispatcher's `Sync` branch where the import actually happens:
`LoroShareBackend` now holds the `WriteTierAuthority` and calls
`adopt_imported_block(s)` before each of its three SQL-projection writes (the
live peer-sync projection worker, the accept/rehydrate descendant projection,
and the mount node, whose accept parent can itself be a read-only-homed block).

Covered by `a_peer_import_under_a_read_only_homed_block_is_adopted_not_left_editable`
in `crates/holon-loro/src/loro_share_backend.rs`, which drives the real
projection worker with a peer's update and asserts the import lands, keeps the
recipe step as its parent, and earns the file's refusal.

Still not routed through the dispatcher: the whole-store pairing re-import
(`DevicePairing::reimport` → `BlockOrdering::create_in_tree_batch` →
`BlockCellRegistry`) writes to Loro through a third seam that consults no
write-tier authority on create. Out of scope here and unpinned.

**2. The adoption is session state, and its loss is silent.** It is not written
into `file.read_only_blocks` (that column is the FILE's account of itself,
stamped by the ingest leg), so a reboot loads the file's blocks and not the
imported one, and the imported block is editable again. Nothing announces
that: the only disclosure is the WARN emitted at ADOPT time, in the session
that did the importing — the next boot says nothing at all, and the user sees
an editable block with no banner. Persisting it needs the sharing leg to own a
store of its own imports; recorded here rather than bolted onto the file row,
which would make the ingest leg the authority for blocks no file declares.
