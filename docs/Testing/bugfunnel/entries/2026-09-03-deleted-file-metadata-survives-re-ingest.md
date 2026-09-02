---
id: 2026-09-03-deleted-file-metadata-survives-re-ingest
date: 2026-09-03
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A property the user deleted from a vault file stayed on its document block
  forever, so the page kept serving metadata the file does not have.
---

## Bug

A file is ingested carrying `servings: 4` and `source: Familienrezept`. The
user deletes the `source` line and saves. The next ingest still shows
`source: Familienrezept` on the document block — no error, no banner, no way to
remove the key short of deleting the page.

Found by the fresh-context verifier of lane `ingest-contract`, driving the real
dispatcher (`FileSyncController::on_file_changed`) rather than the pure
function: expected `doc.get_property_str("source") == None`, got
`Some("Familienrezept")`. Verdict `ingest-contract-verify.md`, defect D1.

Priority-order class 4: the store silently degrades to look fine.

## Root cause

`holon_core::apply_document_metadata`
(`crates/holon-core/src/file_format.rs`) — the default body of
`FileFormatAdapter::sync_document_metadata`, and therefore the ingest contract
every format without an override rides — only INSERTED and UPDATED. It walked
`parsed.properties` and wrote each key onto the persisted block; a key the
persisted block held and the parse no longer declared was never visited, so it
survived every re-ingest.

The asymmetry was the tell: org's own override
(`crates/holon-orgmode/src/file_format.rs:175-188`) already matched on
`Option` and removed `FILE_ID_KEYWORD` when the parse dropped it. The removal
case was known at the seam and not carried into the generic default, so every
format WITHOUT an override — cooklang, LogSeq markdown, Obsidian markdown — got
the one-way version.

## Missing piece

The keystone registers org only, and org overrides
`sync_document_metadata` — so the defective default body does not run in the
keystone's wiring at all, whatever the keystone generates (ENVIRONMENT). Behind
that, no transition edits a document's METADATA on disk and re-ingests: the
file-edit transitions change block text, so even with a second format
registered the removal case would not be generated (COVERAGE).

## Remedy

`apply_document_metadata` now reconciles the persisted property bag to EXACTLY
what the parse declares — `parsed.properties` plus the declared title under
`DOCUMENT_TITLE_KEY` — removing every key the file no longer carries. The org
override stays: it routes the same two things through `#+TITLE:` and the
file-level `:PROPERTIES:` drawer, and carries the doc-root body and
`todo_keywords` besides, so it is not redundant.

Covering test:
`re_ingest_removes_metadata_the_file_no_longer_declares`
(`crates/holon-orgmode/tests/ingest_contract.rs`), driven through
`on_file_changed` with a `.fixture` adapter whose declared metadata the test
edits between ingests.

Red log: `lane-logs/r3-d1-RED-041329.log` —
`left: Some("Familienrezept") / right: None`.
