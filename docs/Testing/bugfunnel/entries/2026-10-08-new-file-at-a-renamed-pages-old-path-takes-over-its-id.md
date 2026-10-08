---
id: 2026-10-08-new-file-at-a-renamed-pages-old-path-takes-over-its-id
date: 2026-10-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  After doc_904.org is renamed to doc_905.org, a new doc_904.org silently
  upserts onto the renamed page's id, so no page doc_904 appears and the
  renamed page is overwritten.
---

## Bug
Found by agent exploration (recreate lane, 2026-10-08) with the sequence
CreateDocument doc_904.org, RenameDocument doc_904.org to doc_905.org,
CreateDocument doc_904.org. The SUT times out waiting for a page titled doc_904.
No error and no disclosure.

## Root cause
The new file has no `#+ID`, so ingest resolves its page through
`resolve_dir_page_chain` (crates/holon-filesystem/src/file_sync_controller.rs).
No page is titled doc_904 under the root, so it minted
`PageId::for_path("doc_904")`, but the renamed page keeps that id
(PageIdentityDeterminism.md §5.3), and `create_forcing_id` upserted onto it.
A temporary probe measured `intended_id=block:f3b1d47c…
existing_holder=Some((block:f3b1d47c…, "doc_905"))`
(lane-logs/recreate/probe-p2-idprobe.log).

Observation, not fixed here: the rename ran as delete plus recreate.
`poll_tracked_files` saw the old path gone before `on_file_renamed`, which then
logged "source had no known document; ingesting the destination as a new file"
(lane-logs/recreate/probe-p2.log).

## Missing piece
The keystone `CreateDocument` generator always names a fresh `doc_<n>.org`, so
no generated sequence creates a file at a path that a rename or delete vacated.

## Remedy
FIXED. Every page-creating path takes its id from `holon_api::page_slot`
(crates/holon-api/src/identity_recognition.rs): `PageId::for_path(path)` when it
is free, else the first free id along `PageId::for_path_beside(path, held_id)`
(crates/holon-api/src/link_parser.rs), with each passed holder disclosed by a
`warn!`. The callers are org-file ingest and the default
`DocumentManager::get_or_create_by_name_chain` (both through
`DocumentManager::page_slot`) and a dangling-link click
(`SqlOperationProvider::resolve_destination_chain`). Write-back stores the id as the file's `#+ID`.
PageIdentityDeterminism.md §5.3 describes the rule and when two peers can still
mint different ids.

Hand-authored row `a-new-file-at-a-renamed-pages-old-path-is-a-new-page`: green
(lane-logs/recreate3/d5-green.log). The `create_document` driver asserts that a
new file takes no tracked file's page and changes none; with ingest put back on
bare `PageId::for_path` the row goes red on that assertion,
"doc_904.org took the page block:f3b1d47c… of the tracked file doc_905.org"
(lane-logs/recreate3/d5-teeth.log:59). Red, green and teeth for the link click and
the trait default: lane-logs/recreate3/red-d2.log, red-d3-d4-engine.log,
green-cpfl-run*.log, teeth-d2.log, teeth-d3-linkclick.log.

## Known limits
- A `[[name]]` link whose `block_links` row resolved while the page still had
  that name keeps pointing at the renamed page: no write path re-resolves the
  junction when a page is renamed. Links written after the rename resolve by
  name. Pinned by the ignored test
  `a_link_resolved_before_a_rename_resolves_to_the_page_with_that_name`
  (crates/holon/tests/create_page_from_link.rs).
- `convert_block_to_page` still refuses its new page's id when a renamed page
  holds it (`IdentityCollision`).
- The journal rule still skips a day whose derived id a renamed journal page
  holds. Creating the day's page beside it instead makes the rule mint a page
  whenever it re-evaluates (boot, rule edits, clock moves), which the keystone
  reference does not model (lane-logs/recreate3/gate-rows.log, row
  `main-panel-drops-refocused-split-block`).
- Open: the generator still does not re-create vacated file names.
