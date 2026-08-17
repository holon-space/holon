---
id: 2026-07-26-renaming-org-file-does-rename-page
date: 2026-07-26
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Renaming an org file does NOT rename the page (identity audit, found
  accidentally while investigating the page-id collision above, and
  INDEPENDENT of it): renaming `A.org` → `B.org` on disk with the `#+ID:`
  carried across leaves the page titled "A" forever. Root cause:
  holon-filesystem/src/file_sync_controller.rs:1479-1483 — when `#+ID:`
  resolves to an EXISTING doc the controller takes the `(doc, false)` arm and
  never applies the filename-derived title to that existing page, so the
  on-disk name and the projected page title diverge permanently with no error.
source_line: 1109
---

## Bug

Renaming an org file does NOT rename the page (identity audit, found
accidentally while investigating the page-id collision above, and
INDEPENDENT of it): renaming `A.org` → `B.org` on disk with the `#+ID:`
carried across leaves the page titled "A" forever. Root cause:
holon-filesystem/src/file_sync_controller.rs:1479-1483 — when `#+ID:`
resolves to an EXISTING doc the controller takes the `(doc, false)` arm and
never applies the filename-derived title to that existing page, so the
on-disk name and the projected page title diverge permanently with no error.

## Root cause

renaming an org file does NOT rename the page (identity audit; independent
of the collision above) — `A.org`→`B.org` with `#+ID:` carried across leaves
the page titled "A"; `file_sync_controller.rs:1479-1483` takes the `(doc,
false)` existing-doc arm and never applies the filename-derived title. Same
structural blocker: the catalog has create/delete/external-write but NO
file-rename rung, so a doc's path is fixed for its lifetime and this state
is never entered.)

## Missing piece

Same structural blocker as the row above: no transition in the catalog
renames a document. The catalog has create (`create_document.rs`), delete
(`delete_document.rs`) and external write (`write_org_file.rs`) but no
file-rename rung, so a doc's path is fixed for its whole lifetime and the
`#+ID:`-resolves-to-existing-doc-under-a-new-filename state is never
entered. Missing piece: a RenameDocument transition on the external rung (mv
the file, keep `#+ID:`) plus a reference expectation that the page title
tracks the filename.

## Remedy

OPEN (fix = apply the filename-derived title on the existing-doc arm, gated
behind the new rename transition going red first)
