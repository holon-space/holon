---
id: 2026-10-05-text-undo-commits-under-loros-default-origin
date: 2026-10-05
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  TextUndo::undo and redo committed with Loro's default origin "undo" instead of
  sys.ui_undo, so origin-keyed subscribers and filters could not tell a text
  undo from any other writer.
---

## Bug

Found by the D68 spike (lane `batch-undo`) while probing which origins a Loro
`UndoManager` leaves on its commits. Found outside an automated test.

## Root cause

`TextUndo::undo` and `redo` (`crates/holon-loro/src/text_undo.rs`) called the
`UndoManager` without arming the document's write origin, so the commit carried
Loro's default origin string `"undo"`. Every other write path commits under a
`WriteOrigin`.

Measured: `lane-logs/bu-RED-a5-undo-origin.log` shows
`left: ["undo", "undo"] right: ["sys.ui_undo", "sys.ui_undo"]`.

## Missing piece

**ORACLE.** No test asserted the origin of a text undo or redo commit, and no
invariant requires every commit to carry a `WriteOrigin` string.

## Remedy

`WriteTxn::arm_origin()` (`crates/holon-loro/src/loro_document.rs`, next to
`commit`) sets the origin before an undo manager commits; `TextUndo::undo` and
`redo` call it. Pinned by `undo_and_redo_commit_under_the_ui_undo_origin`
(`crates/holon-loro/tests/text_undo_contract.rs`). Red:
`lane-logs/bu-RED-a5-undo-origin.log`. Green:
`lane-logs/bu-GREEN-a5-undo-origin.log` (4 passed).
