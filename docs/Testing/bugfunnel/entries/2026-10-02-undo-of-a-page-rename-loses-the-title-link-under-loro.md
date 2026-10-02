---
id: 2026-10-02-undo-of-a-page-rename-loses-the-title-link-under-loro
date: 2026-10-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Under Loro CRUD authority, undoing a page rename restored the old title text but not the link
  marks on it, so the authored link was lost for good.
---

## Bug

Rename a page whose title is a link, then `UndoLastMutation`, with Loro+Turso wiring: content came
back as `pagea`, marks stayed `None` (reference: `Link 0..5`). Turso-only wiring restored both.
Found by the adversarial verifier of the page-rename-marks lane (probe sidecar
`lane-logs/prmv-sidecar.jsonl`).

## Root cause

The plain-String `content` arm of `LoroBlockOperations::set_field` inverted with
`Value::String(prior.content)` (no marks). The dispatcher's marks follow-up clears the marks in the
same undoable step, so a text-only inverse cannot restore them. The SQL provider already builds
the rich `{text, marks}` inverse (`sql_operation_provider.rs`, content arm).

## Missing piece

No keystone case undid a rename of a mark-bearing title under Loro authority.

## Remedy

The Loro plain-String arm now inverts with `rich_content_restore_value`. Cases
`undo-of-a-page-rename-restores-the-title-link-under-loro-authority` (and its Turso twin) in
`hand-authored-regressions/keystone.jsonl`; unit test
`set_field_plain_content_inverse_restores_marks`.

Evidence: red `lane-logs/prm2-red.log` (`marks: sut=None ref=Some([Link 0..5])`), green
`lane-logs/prm2-green-*.log` (9 passed each).
