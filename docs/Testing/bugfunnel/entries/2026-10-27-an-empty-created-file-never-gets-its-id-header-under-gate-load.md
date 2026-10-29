---
id: 2026-10-27-an-empty-created-file-never-gets-its-id-header-under-gate-load
date: 2026-10-27
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Under gate load, an empty org file created on disk is ingested as a page, but its `#+ID:`
  header write never lands, so the file stays 0 bytes and the page owns no file.
---

## Bug
`holon-integration-tests::two_instance_composed_pbt
joining_a_received_page_away_is_refused_and_leaves_it_placed` was red once in the extra
nextest leg of an integrate gate run (holon-space/holon-work#781). The oracle reported two
invariants at once:

- `inv-org-render-fixed-point`: disk `doc_0.org` is 0 bytes; the render from SQL is the
  43-byte `#+ID: <uuid>` header; the difference persisted for 5 s.
- `inv-every-page-has-its-own-file`: the page owns NO file.

Evidence: `docs/Testing/known-red-evidence/empty-created-file-header-never-written/gate-excerpt.log.zst`.
A 0-byte file next to a page that the store holds can lose data on a later write, so it is a
real defect, not only test noise.

## Root cause
The header write lands. A later write-back overwrites it with an empty render:

- The ingest of the empty `doc_0.org` writes the 43-byte `#+ID:` header.
- Sharing the page moves it out of the global Loro tree
  (`crates/holon-loro/src/shared_tree.rs:649`, `commit_share_prune`). The incremental pass of
  the global projection saw the page leave the tree and emitted a `delete` of its row in the
  write authority (`block_raw`). `project_descendants_to_sql` re-created the row about 16 ms
  later (`crates/holon-loro/src/loro_share_backend.rs`, `share_subtree`).
- In that window no store held the page: the holder, the page registry
  (`DocumentManager::get_by_id`) and the write authority
  (`BlockReader::get_block_authoritative`) all lacked it. So `WritebackRenderer::render_blocks`
  (`crates/holon-filesystem/src/writeback_render.rs:89`) rendered body-only, `""`, as it does
  for a deleted page, and the write-back net wrote `""` over the file.
- A later change usually wrote the header back. When none followed, or the watcher event of the
  empty file reached the empty-file refusal first, the file stayed 0 bytes and the page owned
  no file.

## Missing piece
The failing path ran in almost every run of the test (a probe build logged the 0-byte write in
159 of 160 runs), but the oracle only saw it when no later write restored the header: the test
passed 25 of 26 gate extra runs. No test asserted that a share leaves the page's row in
`block_raw`.

## Remedy
- A share moves rows; it deletes none. The incremental pass of the global projection
  (`crates/holon-loro/src/loro_sync_controller.rs:1263`, `:1296`) emits no `delete` for a row
  that a loaded share holds (`registered_share_rows`), the rule its full walk already had.
- `share_subtree` registers the shared doc before the prune commits
  (`crates/holon-loro/src/loro_share_backend.rs:2170`), so the first projection pass over the
  prune already counts the moved rows as the share's.
- Red-first test `sharing_a_page_deletes_none_of_its_rows`
  (`crates/holon-integration-tests/tests/loro_suite/loro_projection_share_rows.rs`): red before
  the fix (the page and child rows deleted), green after.
- Probe build of the two-instance test: 0 zero-byte writes of `doc_0.org` in 160 runs after
  the fix, 159 of 160 with the share fix reverted.
- The page registry lags the holder and the write authority, so the write-back no longer
  depends on it alone for the header:
  - `render_doc_from_holder` (`crates/holon-filesystem/src/file_sync_controller.rs:8937`) takes
    the header from a held page root; pinned by
    `crates/holon-orgmode/tests/a_lagging_page_registry_keeps_the_page_header.rs`.
  - `WritebackRenderer::render_blocks` takes it from the page registry, else from the write
    authority; pinned by
    `crates/holon-orgmode/tests/a_retracted_page_root_keeps_the_page_header.rs`, which is red
    without it, also with the share fix in place.
- Removed the known red `empty-created-file-header-never-written` from
  `docs/Testing/KeystoneKnownReds.md`.
