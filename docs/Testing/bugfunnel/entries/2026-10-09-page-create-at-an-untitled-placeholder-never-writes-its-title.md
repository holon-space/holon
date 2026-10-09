---
id: 2026-10-09-page-create-at-an-untitled-placeholder-never-writes-its-title
date: 2026-10-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Ingest creating a page at an id held by an untitled Page placeholder returned
  the placeholder unchanged, so the page never got its title.
---

## Bug
Found by code audit (recreate-lane round-3 verifier, defect C). `page_slot`
answers `Create` for an id held by an untitled placeholder, promising the create
completes it, but `LiveDocumentManager::create_forcing_id`
(crates/holon-app/src/turso_seams.rs) returned any existing row at the id.
Reached from `resolve_dir_page_chain`
(crates/holon-filesystem/src/file_sync_controller.rs).

## Root cause
`create_forcing_id` early-returned `get_by_id(doc.id)` whenever it was `Some`,
titled or not. Reproduced: crates/holon-app/tests/page_slot_completes_a_placeholder.rs
red with the returned title `""` instead of `"Music"`
(lane-logs/recreate4/red-c.log).

## Missing piece
No generated sequence leaves an untitled `Page` row at a path-derived id before
a file for that path is ingested.

## Remedy
FIXED. `create_forcing_id` completes an untitled placeholder through
`update_in_tree` (`complete_placeholder`) and refuses one that sits under
another parent. Green: lane-logs/recreate4/green-c.log.
