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

## Residual — two, both open and both disclosed here only

**1. The Loro import leg does not reach the adoption at all.** The write-tier
gate keys on `OpOrigin`, and `LoroShareBackend`'s import calls
`execute_operation` — the origin-less entry point, which defaults to
`OpOrigin::User` — at `crates/holon-loro/src/loro_share_backend.rs:808`, `:845`
and `:935`. So the path a real peer's blocks will travel does not currently pass
`OpOrigin::Sync`, and `adopt_sync_import` is bypassed. Two consequences, and
they point opposite ways: today those imports are judged as USER writes, so a
block created under a read-only root is REFUSED outright (the store stays
consistent, the peer's block is dropped); once that leg is given its true
origin, adoption starts running and the block lands uneditable instead. The fix
in this entry is therefore correct at the gate and inert on the only import leg
that exists — closing that requires the sharing lane to thread the origin
through, which is its call, not this one's.

**2. The adoption is session state, and its loss is silent.** It is not written
into `file.read_only_blocks` (that column is the FILE's account of itself,
stamped by the ingest leg), so a reboot loads the file's blocks and not the
imported one, and the imported block is editable again. Nothing announces
that: the only disclosure is the WARN emitted at ADOPT time, in the session
that did the importing — the next boot says nothing at all, and the user sees
an editable block with no banner. Persisting it needs the sharing leg to own a
store of its own imports; recorded here rather than bolted onto the file row,
which would make the ingest leg the authority for blocks no file declares.
