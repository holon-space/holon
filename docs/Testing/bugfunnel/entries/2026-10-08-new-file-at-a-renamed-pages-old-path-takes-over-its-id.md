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
FIXED. `FileSyncController::free_page_id` gives a new page `for_path(path)`
when it is free; when another page holds it, it follows
`PageId::for_path_beside(path, held_id)` (crates/holon-api/src/link_parser.rs)
to the first free id. Write-back stores that id as the file's `#+ID`.
PageIdentityDeterminism.md §5.3 describes the rule.

Hand-authored row `a-new-file-at-a-renamed-pages-old-path-is-a-new-page`: red
before the fix (lane-logs/recreate/red2-red.log), green after
(lane-logs/recreate/red2-green.log). Teeth: `path_id` put back in place of
`free_page_id` turns it red with the same timeout
(lane-logs/recreate/red2-teeth.log).

Open: the generator still does not re-create vacated file names.
