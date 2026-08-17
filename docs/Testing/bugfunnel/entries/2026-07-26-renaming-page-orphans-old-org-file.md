---
id: 2026-07-26-renaming-page-orphans-old-org-file
date: 2026-07-26
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Renaming a PAGE orphans its old org file — the page ends up DOUBLE-HOMED
  across two files (identity audit; surfaced by PR #99's new keystone rename
  machinery on random walks; the write-back CONVERSE of the
  org-file->page-title break in the row above, and INDEPENDENT of the
  collision two rows up). After renaming page A->B (title "Renamed") via the
  production `set_field` op, the org writeback CREATES the new file
  `.../structural-page/Renamed.org` but NEVER removes the old
  `.../structural-page/pagea.org`, and BOTH files still carry `#+ID:
  a6bbdb1d-...`. So the page `a6bbdb1d-...` is rooted in TWO files at once,
  where exactly one `#+ID:` file must root it. This is the writeback converse
  of the D2 defect (`file_sync_controller.rs:1479-1483`): one direction of the
  page-title<->file-name binding never renames the page, this direction never
  deletes the stale file — both directions broken.
source_line: 1111
---

## Bug

Renaming a PAGE orphans its old org file — the page ends up DOUBLE-HOMED
across two files (identity audit; surfaced by PR #99's new keystone rename
machinery on random walks; the write-back CONVERSE of the
org-file->page-title break in the row above, and INDEPENDENT of the
collision two rows up). After renaming page A->B (title "Renamed") via the
production `set_field` op, the org writeback CREATES the new file
`.../structural-page/Renamed.org` but NEVER removes the old
`.../structural-page/pagea.org`, and BOTH files still carry `#+ID:
a6bbdb1d-...`. So the page `a6bbdb1d-...` is rooted in TWO files at once,
where exactly one `#+ID:` file must root it. This is the writeback converse
of the D2 defect (`file_sync_controller.rs:1479-1483`): one direction of the
page-title<->file-name binding never renames the page, this direction never
deletes the stale file — both directions broken.

## Root cause

renaming a PAGE orphans its old org file — the page ends up DOUBLE-HOMED
across TWO files (identity audit, surfaced by PR #99's new keystone rename
machinery on random walks). After renaming page A->B (title "Renamed") via
the production `set_field` op, the org writeback CREATES the new
`.../structural-page/Renamed.org` but NEVER removes the old
`.../structural-page/pagea.org`, and BOTH files still carry `#+ID:
a6bbdb1d-...`, so `inv-every-page-has-its-own-file` sees one page rooted in
two `#+ID:` files. Write-back CONVERSE of the D2 org-file->page-title break
— one direction never renames the page, this direction never deletes the
stale file; both halves of the page-title<->file-name binding broken.
COVERAGE primary: TEMPORALLY unreachable by the pre-#99 catalog (no
transition renamed a page). Deterministic red ALREADY EXISTS but PARKED —
`inv-every-page-has-its-own-file` fires at transition 5/6 (RenamePage) of
hand-authored case `page-id-rename-collision` on bookmark
keystone-rename-detect (rev c5da2a4e708e); evidence
`/private/tmp/holon-keystone-rename-detection.md`; prod fix pending.)

## Missing piece

TEMPORALLY unreachable by the pre-#99 catalog: no transition renamed a page
(`focus_editable_text.rs:174` and `apply_mutation.rs` both exclude page
blocks from every editing ingress), so the rename->stale-file state was
never entered. Closed as reachable by PR #99's `RenamePage` transition,
which drives the production `set_field` op. Deterministic detection ALREADY
EXISTS: invariant `inv-every-page-has-its-own-file` fires at transition 5/6
(`RenamePage`) of the PARKED hand-authored case `page-id-rename-collision`
in `hand-authored-regressions/keystone.jsonl` on bookmark
`keystone-rename-detect` (rev c5da2a4e708e, comment block ~lines 224-266);
evidence report `/private/tmp/holon-keystone-rename-detection.md`. Parked
because it reds one tick BEFORE the collision, on this independent defect.

## Remedy

OPEN — red detection EXISTS (parked keystone case, red-for-the-right-reason
at transition 5/6), prod fix PENDING = the writeback must delete the old org
file when a page is renamed (or refuse to leave two `#+ID:` roots).
