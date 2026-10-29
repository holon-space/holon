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
- Sharing the page re-projects it. The resulting change of the page reaches
  `on_block_changed` (`crates/holon-filesystem/src/file_sync_controller.rs:8574`), which
  renders from the holder.
- The render asked the page registry (`DocumentManager::get_by_id`) for the page's header.
  Production's registry (`LiveDocumentManager`, `crates/holon-app/src/turso_seams.rs`) is a
  matview of `Page`-tagged blocks fed by its own CDC stream, so it can lag the holder.
  When it lacked the page, `WritebackRenderer::render_blocks`
  (`crates/holon-filesystem/src/writeback_render.rs:89`) rendered body-only: `""`.
- The write-back net wrote `""` over the file. A probe build of the two-instance test logged this
  0-byte write in 157 of 160 runs; a later change usually wrote the header back. When none
  followed, or the watcher event of the empty file reached the empty-file refusal first, the
  file stayed 0 bytes and the page owned no file.

## Missing piece
The failing path runs in the test, but only a registry lag at the moment of the share exposes
it as a lasting failure: the test passed 25 of 26 gate extra runs and was red 1 of 128 and
0 of 160 times (our shape) in local batches. No test forced the registry to lag the holder.

## Remedy
- `render_doc_from_holder` (`crates/holon-filesystem/src/file_sync_controller.rs:8852`) renders
  the header from the page root the holder holds, and asks the registry only when the holder
  has no page root.
- Red-first test `crates/holon-orgmode/tests/a_lagging_page_registry_keeps_the_page_header.rs`
  removes the page from the registry before a page change and asserts the `#+ID:` header
  stays on disk: red before the fix (disk `""`), green after.
- Removed the known red `empty-created-file-header-never-written` from
  `docs/Testing/KeystoneKnownReds.md`.
