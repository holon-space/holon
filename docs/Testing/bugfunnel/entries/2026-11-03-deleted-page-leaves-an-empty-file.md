---
id: 2026-11-03-deleted-page-leaves-an-empty-file
date: 2026-11-03
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A deleted page (also the undo of convert-to-page) left its .org file in the
  vault with 0 bytes, and its path records stayed.
---

## Bug
A verifier found it while it checked the undo of convert-to-page: after the
undo, the page's file stayed in the vault and held 0 bytes. The path records
(alias, doc home, last projection) of the file stayed too.

## Root cause
The catch-up write-back renders a page that the store no longer holds as ""
and writes those 0 bytes like any other render
(`crates/holon-filesystem/src/file_sync_controller/writeback.rs`,
`catch_up_write`, then `write_back_or_skip_readonly`). Only the materialize path
skipped an empty render. No path removed the file of a deleted page.

Evidence: `crates/holon-orgmode/tests/a_deleted_page_leaves_no_file.rs` is red
in its fixed cases with "the deleted page's file Origin.org still exists,
holding \"\"" before the fix.

## Missing piece
The keystone deletes pages (DeleteDocument, UndoLastMutation of BlockToPage),
but `inv-every-page-has-its-own-file` only checked that each page of the
reference has one file. No invariant checked the converse: that every file in
the vault roots a block the reference holds. The SUT's org-render snapshot
also left out a vault file with no `#+ID` or no store block, so even such an
invariant could not see the file.

- Litmus COVERAGE: yes, the catalog reaches the state.
- Litmus ORACLE: no invariant went red on it.

## Remedy
- `inv-every-page-has-its-own-file` now also fails on a file that is empty or
  that roots a block the reference does not hold.
- The SUT's org-render snapshot (`frontend_slice/components.rs`) now reports
  every vault file, also one without a `#+ID` or a store block.
- The catch-up write-back removes the file of a page the store no longer
  holds and forgets its path records (`remove_deleted_page_file` in
  `writeback.rs`). It refuses a file another page owns, a file it did not
  write, and a read-only file; a path outside the vault never resolves.
- A file that the user emptied by hand keeps its behaviour (`accept_removal`).
