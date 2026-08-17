---
id: 2026-07-20-edit-mode-block-shows-stripped-link
date: 2026-07-20
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Edit-mode block shows STRIPPED link label — raw `[[…]]` delimiters invisible
  while editing (user report): stored `content` never holds bracket syntax
  (dispatcher strips via `extract_inline_marks`,
  `operation_dispatcher.rs:632-659`; org ingest likewise `parser.rs:397`);
  editor seeds `InputState` from stripped content
  (`editor_view.rs:216-221,426`); the inverse serializer `render_inline_marks`
  (`inline_marks.rs:326`) is only called for org writeback + PBT oracle, never
  the editor. No raw-vs-rendered mode exists, so the user cannot see what is
  inside/outside a reference while editing.
source_line: 1043
---

## Bug

Edit-mode block shows STRIPPED link label — raw `[[…]]` delimiters invisible
while editing (user report): stored `content` never holds bracket syntax
(dispatcher strips via `extract_inline_marks`,
`operation_dispatcher.rs:632-659`; org ingest likewise `parser.rs:397`);
editor seeds `InputState` from stripped content
(`editor_view.rs:216-221,426`); the inverse serializer `render_inline_marks`
(`inline_marks.rs:326`) is only called for org writeback + PBT oracle, never
the editor. No raw-vs-rendered mode exists, so the user cannot see what is
inside/outside a reference while editing.

## Missing piece

The `inv-displayed-text` family (`text_compare.rs:9-11`) compares editor
text to a reference that re-mirrors the same stripped label
(`ref_caps/editor.rs:25-31`) — structurally can never flag this. Missing
invariant: `active_editor_text == render_inline_marks(content, marks)`
whenever a focused block carries marks (RED today). Remedy: model-first —
reference editor holds raw text, prove red, then Logseq-style raw edit mode
(seed from `n(content, marks)` on focus, dispatcher's existing extraction
handles commit; round-trip already proptested).

## Remedy

OPEN 2026-07-20 (user report)
