---
id: 2026-09-11-a-sync-import-projected-before-its-document-is-recorded-stays-editable
date: 2026-09-11
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A peer's block projected under a read-only-homed document BEFORE the file-sync
  controller records that document's home is never re-judged, so it stays fully
  editable for the rest of the vault's life with no disclosure.
---

## Bug

Found by the fresh-context verifier of lane `loro-import-origin` as an unprobed
hypothesis, and confirmed by a test written for it (outside any pre-existing
test), so it is a reading finding rather than a dogfood one.

The share projection leg adopts an imported block by asking
`WriteTierAuthority::adopt_sync_import`, which resolves the parent against
`ReadOnlyDocuments`. Both the ordering and the registry work as designed. The
gap is between them: the registry is filled by the file-sync controller as it
records each document's home, and a share can project before that has happened
for the document in question — a share attached at boot, or a vault whose
ingest has not reached the `.cook` file yet.

At projection time the parent belongs to no recorded document, so `adopt`
correctly answers `false` and binds nothing. When the ingest later calls
`record`, the membership it installs is derived from the FILE, and the file
never declares an imported block. Nothing re-judges the import. The block keeps
the full editing affordance set, every edit to it is accepted, and none of them
can ever reach the authoritative file.

## Root cause

The adoption is decided ONCE, at the moment of import, against a registry that
is still being filled. There is no second decision point, and no component owns
the question "which blocks did this device import" in a form `record` could
consult.

## Missing piece

COVERAGE. Every write-tier test — the dispatcher's, the gate's own, and the
share backend's new ones — records the document FIRST and imports second. That
ordering is the one the tests happened to be written in, not one the system
guarantees. No test drove the reverse order, so the gap was never exercised.

## Why it is not fixed here

`ReadOnlyDocuments` holds no block tree. It maps block ids to documents and
cannot enumerate the children an earlier import placed under the blocks
`record` is now claiming, so the fix cannot live inside `record`.

Closing it needs an owner for "imports this device has taken" that survives
independently of any file's account of itself — which is the same store that
Residual #2 of
`2026-09-08-a-synced-block-under-a-read-only-document-stays-editable` asks for
(adoption is session state today and is lost on reboot, also silently). The two
should be closed together by whoever owns the sharing leg's persistence.

## Pinned by

`an_import_projected_before_its_document_is_recorded_is_never_re_judged` in
`crates/holon-loro/src/loro_share_backend.rs`, marked `#[ignore]` with this
entry's id so the suite stays green while the case stays runnable:

    cargo nextest run -p holon-loro --run-ignored all \
      an_import_projected_before_its_document_is_recorded

It fails on the final assertion — the imported block earns no refusal after the
recipe is recorded — which is the bug exactly.
