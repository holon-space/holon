---
id: 2026-10-28-a-deleted-org-file-comes-back-from-a-stale-page-mirror
date: 2026-10-28
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  An org file the user deleted outside Holon can come back: a late block change of its page reaches the write-back while the page mirror still lists the page, so the write-back finds the old path and writes the file again.
---

## Bug
The hand-authored keystone case `deletion-undone-stands-when-a-copy-file-is-deleted-sqlonly-arm` goes red under load at `crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:6067`: `gamma.org exists again after its blocks vanished`. On base b213f46d1edf, alone, 2 of 96 runs at 24 parallel runs (1-min load 59-66), 0 of 40 at 8 parallel runs (load ~50). In both red runs the write-back logs `Wrote …/gamma.org` for `block:ref-doc-2` about 20 ms after `on_file_deleted` cascade-deleted that page. Green runs never write gamma.org after the deletion.

## Root cause
- `FileSyncController::on_file_deleted` (crates/holon-filesystem/src/file_sync_controller.rs:2262) removes the page from the store and forgets the file state (`forget_file_state`, :3154, sets the net's ON_DISK to ABSENT).
- A block change of that page that arrives afterwards runs `on_block_changed_memoized` (:8458). It finds the path with `doc_id_to_path(…, WriteOwnFile)` (:8505), which asks `DocumentManager::name_chain` → `get_by_id`.
- The production `LiveDocumentManager::get_by_id` (crates/holon-app/src/turso_seams.rs:807) answers from its `live` mirror. Only the matview CDC stream removes a deleted page from that mirror, and it runs apart from the block-change stream. So the mirror still has the page and gives `gamma.org`.
- `catch_up_write` (crates/holon-filesystem/src/file_sync_controller/writeback.rs:328) sees disk ABSENT equal to the net's ABSENT and writes the file again.

## Missing piece
Litmus ENVIRONMENT: the keystone runs this path, but the two CDC streams reorder only under scheduler pressure, so the keystone is red in a few percent of loaded runs. Litmus COVERAGE (secondary): no test let a block change of a deleted page arrive before the page mirror dropped the page. Pinned by crates/holon-orgmode/tests/deleted_page_is_not_rewritten_from_a_stale_page_mirror.rs (a stale `DocumentManager` and an empty store), red on the base.

## Remedy
Open. The fix belongs in crates/holon-filesystem: the write-back must not find a path for a page that the authoritative store no longer holds (or that the controller cascade-deleted), whatever the page mirror says. Until then the known-red row `deleted-file-recreated-by-stale-page-mirror` (docs/Testing/KeystoneKnownReds.md) holds the red.
