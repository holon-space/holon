---
id: 2026-07-19-undo-leaves-inconsistent-content-marks-pair
date: 2026-07-19
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  UNDO leaves an inconsistent (content, marks) pair (Mac dogfood, SqlOnly):
  editing a block that ALREADY carries a link mark (e.g. `See [[Old Page]]`)
  with a further content edit, then a single `undo`, reverts `content` to the
  predecessor text but DROPS the surviving link mark (marks column → NULL) —
  the `[[…]]` link is silently destroyed; `redo` restores it. Root cause:
  `SqlOperationProvider::execute_operation_with_origin`'s
  `set_field("content")` arm built a CONTENT-ONLY String inverse
  (`crates/holon/src/core/sql_operation_provider.rs`, old
  `set_field_inverse(id, field, old_value)`), while the dispatcher folds the
  DERIVED `marks` write into the SAME undoable step with no undo entry of its
  own (`operation_dispatcher.rs:761-781`, "one user edit = one undoable
  content step"). So the single undo entry could not restore the predecessor's
  marks: on undo replay the derived-marks follow-up re-derives marks from the
  restored (stripped, `[[…]]`-free) content → empty → the surviving link mark
  is cleared. (Symmetric over-retention exists for a text-unchanged mark-add,
  but that content write is vacuous and the engine never journals it, so it is
  not an engine-undoable case.)
source_line: 1020
---

## Bug

UNDO leaves an inconsistent (content, marks) pair (Mac dogfood, SqlOnly):
editing a block that ALREADY carries a link mark (e.g. `See [[Old Page]]`)
with a further content edit, then a single `undo`, reverts `content` to the
predecessor text but DROPS the surviving link mark (marks column → NULL) —
the `[[…]]` link is silently destroyed; `redo` restores it. Root cause:
`SqlOperationProvider::execute_operation_with_origin`'s
`set_field("content")` arm built a CONTENT-ONLY String inverse
(`crates/holon/src/core/sql_operation_provider.rs`, old
`set_field_inverse(id, field, old_value)`), while the dispatcher folds the
DERIVED `marks` write into the SAME undoable step with no undo entry of its
own (`operation_dispatcher.rs:761-781`, "one user edit = one undoable
content step"). So the single undo entry could not restore the predecessor's
marks: on undo replay the derived-marks follow-up re-derives marks from the
restored (stripped, `[[…]]`-free) content → empty → the surviving link mark
is cleared. (Symmetric over-retention exists for a text-unchanged mark-add,
but that content write is vacuous and the engine never journals it, so it is
not an engine-undoable case.)

## Missing piece

The marks ORACLE already existed — `inv-blocks-match-ref/block_raw` compares
the SUT `block_raw` snapshot (marks column parsed, `sut_row_parsing.rs:154`)
against the ref via `holon_pbt_core::block_compare::compare_blocks`, whose
`normalize_block` canonicalizes and compares the `marks` field. The gap was
purely COVERAGE: the editor-driven `TypeChars` path (the ONLY write that
mints a `Link` mark, via `set_field("content")` → dispatcher
`extract_inline_marks`) drew from `typing_text_strategy`, which never
emitted `[[…]]` — so link marks were never created in the keystone and the
marks comparison sat vacuous. (The `[[…]]` arm in `extended_content_arm`
feeds non-editor content strategies that don't route through mark
extraction, so it stored literal `[[…]]` text on both sides — no marks, no
divergence.)

## Remedy

FIXED (`crates/holon/src/core/sql_operation_provider.rs`): the
`set_field("content")` inverse now restores the exact prior (content, marks)
PAIR. When the predecessor carried marks the inverse is a RICH
`content=Object{text,marks}` write (SQL mirror of Loro's
`rich_content_restore_value`); a plain predecessor keeps the String inverse
(preserving the undo coalescer). A new `set_field_content_rich` handles
`content=Object` (writes both columns + re-derives the `block_links`
junction) — and the dispatcher never splits a derived-marks follow-up for an
Object value (`operation_dispatcher.rs:623` reads `value.as_string()`), so
the restored marks are never clobbered. RED→GREEN regression:
`crates/holon/tests/undo_marks_consistency_repro.rs` (vector B
`undo_link_replace_restores_prior_link_mark` was RED: undo yielded `("See
Old Page", [])`, must be `[Link "Old Page" 4..12]`; vector A
`undo_link_add_restores_prior_pair` passes as a self-heal control), driving
the REAL dispatcher+provider and replaying the captured inverse exactly as
`OperationEngine::replay` does. COVERAGE gap CLOSED: `typing_text_strategy`
(`generators.rs`) now emits a `[[label]]` arm, so the keystone's editor path
mints link marks and `inv-blocks-match-ref/block_raw` observes the (content,
marks) pair through undo.
