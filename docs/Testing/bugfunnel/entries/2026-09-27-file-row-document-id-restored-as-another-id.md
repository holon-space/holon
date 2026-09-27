---
id: 2026-09-27-file-row-document-id-restored-as-another-id
date: 2026-09-27
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `file.document_id` held unchecked text and was restored at boot with
  `EntityUri::from_raw`: `":x"` came back as `block::x`, `"a b"`, `"café"` and
  `"a%zz"` panicked, and a `file:` or fragment document was written as its path
  and came back as another `block:` id.
---

## Bug
Found by the adversarial verifier of org-faithful group A, round 5
(`lane-logs/groupA-r5-verify.md`, seam census). The org sync provider stored
the raw `#+ID:` text of every scanned file, including a file the ingest
refuses.

## Root cause
Two writers and one reader of the column with no shared rule:
`orgmode_sync_provider.rs` wrote raw text, `persist_file_projection`
(`crates/holon-app/src/turso_seams.rs`) wrote `uri.id()` (drops scheme,
fragment and query), and `load_file_projections` restored with `from_raw`.

## Missing piece
No test restored a `file` row other than a plain UUID.

## Remedy
`File::document_id_text` / `File::parse_document_id`
(`crates/holon-filesystem/src/file.rs`) are the one codec: a plain bare block
id is stored bare, any other id as its whole URI, and text that names no
document is refused by name. The provider stores only an id that passes the
org id rule. Tests: `crates/holon-app/tests/file_row_document_id.rs` (through
the Turso seam), the provider test
`a_scanned_org_file_whose_id_is_refused_records_no_document`; red
`lane-logs/groupA-r6-red.log`. Rows written by the old code keep their old
reading; a database with such a row is recreated from the vault.
