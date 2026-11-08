---
id: 2026-11-08-an-undecodable-loro-snapshot-is-deleted-silently
date: 2026-11-08
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A Loro snapshot that failed to decode was deleted together with its update
  log, and Holon started on an empty store with only a log line to show it.
---

## Bug
Found by code review while checking whether Holon rebuilds the Loro document
from the org files while a peer exists. Not found by a test.

## Root cause
`LoroDocumentStore::get_doc` (crates/holon-loro/src/loro_document_store.rs)
matched a decode error of the global snapshot and called `remove_file` on the
snapshot and on its update log, with the results ignored. The fresh store then
filled again from the org files. Edits that had reached only the store (for
example ones received from a peer and not yet written back) were lost, and no
notice told the user. Red log: the PBT `a_damaged_snapshot_keeps_its_bytes`
fails with "the snapshot bytes are gone after Flip(..)"; the fixed case
`a_snapshot_of_garbage_keeps_its_bytes` fails the same way.

## Missing piece
- Litmus COVERAGE: no test damaged a stored snapshot; the update-log recovery
  tests damage only the log.
- Litmus ORACLE: no property said that a load keeps the bytes it cannot read.
  The fix adds that property, so ORACLE is primary.

## Remedy
The snapshot and its log are renamed to `<file>.undecodable-<time>` next to
themselves, a rename never replaces an existing file, and a failed rename fails
the load. The condition `LoroSnapshotSetAside` names the moved paths in a
banner. The PBT draws random bytes, a truncation or a flipped byte over a saved
snapshot, with and without log records, and checks that the bytes stay on disk,
that the load succeeds and that a notice names where the files went.
