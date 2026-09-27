---
id: 2026-09-27-value-left-out-of-org-file-only-logged
date: 2026-09-27
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  A stored value the org renderer could not write (a property key the drawer
  cannot hold, a title that reads back as tags, a block id no `:ID:` line
  holds) was only logged; nothing the user sees said the file lacked it.
---

## Bug
Found by the org-faithful plan review (`lane-logs/org-faithful-plan.md` §8),
outside an automated test.

## Root cause
The renderer returned only text. `drawer_line` logged a WARN and dropped the
key; a refused render was an ERROR log in the batch pass.

## Missing piece
No test asserted a user-visible condition for a lossy or refused write-back.

## Remedy
`OrgRenderer::render_document` and `FileFormatAdapter::render_*` return
`Rendered { text, losses }`. The controller's `disclose_render` raises
`WritebackLossy` for the file (subject = the file, detail names the block
and the file) on a loss or a refusal, and clears it on a faithful render.
The engine also refuses a property key the parser reads back as a typed field
(an edge spelling or a storage column). Tests:
`a_value_the_file_cannot_hold_raises_the_files_writeback_condition`
(`crates/holon-app/tests/org_store_org_round_trip.rs`, red
`lane-logs/B-red-writeback-condition.log`) and the typed-key cases in
`editing_suite/drawer_key_write_boundary.rs` (red `lane-logs/B-red-typed-key.log`).
