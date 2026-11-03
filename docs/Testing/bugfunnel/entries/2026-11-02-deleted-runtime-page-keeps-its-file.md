---
id: 2026-11-02-deleted-runtime-page-keeps-its-file
date: 2026-11-02
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A page minted at runtime whose file the ordinary render path wrote keeps
  that file after the page is deleted (undo of convert-to-page, plain delete).
---

## Bug
The org-suite test
`convert_block_to_page_inherits_the_source_ring::a_converted_page_file_declares_its_source_documents_ring`
went red in an integrate gate: after undo of convert-to-page the page row was
gone from the store, but the page file still held the page's `#+ID:` header.
A triage lane reproduced it only under load (6 reds in 384 concurrent runs),
never alone.

## Root cause
- A deleted page has no name chain, so `doc_id_to_path`
  (crates/holon-filesystem/src/file_sync_controller.rs:10419) can route its
  delete only through the alias registrar; without an alias the write is
  skipped and the bulk pass skips it too.
- The only alias registration for a runtime page was in
  `materialize_page_identity_file` (file_sync_controller.rs:9475). That
  function returns early when the ordinary render path already wrote the
  page's file, so a page whose file came from the render path never got an
  alias.
- A second cause: the page registry is a view with its own CDC stream and
  can still list a page the write authority has deleted. The removal's
  render then asked the registry for the page header, so the file kept
  `#+ID:` and `#+TODO:` with no body.
- Load only decided which writer ran first. The order "render path first"
  is deterministic without load: the new controller test is red on the base
  tree for every creation order that starts with the render path, for both
  delete paths.

## Missing piece
No test deleted a page whose file the ordinary render path wrote, or deleted
a page while the registry still listed it; the only test that crossed this
path reached those orders only under load.

## Remedy
- Every write-back that Holon makes registers the alias next to the home
  record, in `write_back_or_skip_readonly`
  (file_sync_controller.rs:10670); the materializer's own registration is
  gone because it writes through the same site.
- A held document whose own root the block feed removed renders without a
  header (`DocOrder::root_removed`, read in `render_doc_from_holder`,
  file_sync_controller.rs); the holder, not the lagging registry, decides.
- Test: crates/holon-orgmode/tests/a_deleted_page_leaves_no_file.rs is a
  property test over creation orders (render, materialize, any sequence),
  both delete paths (undo of convert, plain delete), every arrival order of
  an undo's changes (a plain delete removes leaves first) and every point at
  which the registry drops the page; a live page keeps its file.
- Open: in the SqlOnly wiring the controller has no alias registrar, so a
  deleted page's file stays there for every page, ingested ones included.
