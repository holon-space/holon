---
id: 2026-07-21-editor-stale-buffer-clobber-new-manifestation
date: 2026-07-21
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Editor stale-buffer clobber (new manifestation of the row-80 stale-buffer
  class): after an agent-side `insert_text` op changed a block's content
  MID-FOCUS, the focused GPUI editor still held its PRIOR `InputState` buffer
  and the next keystroke wrote the stale buffer back over the projection — the
  mechanism behind the reported "Enter at line start duplicates block". NOT
  reproducible via MCP-synthesized keys (they take a clean split path); needs
  a real keyboard. Same buffer-authority family as the 2026-07-10 join→stale
  row (fixed only for the focus-gain edge via `converge_on_render`) and the
  2026-07-20 same-seq-echo row, but a DISTINCT trigger neither fix covers: an
  external write arriving WHILE the editor stays focused.
source_line: 1079
---

## Bug

Editor stale-buffer clobber (new manifestation of the row-80 stale-buffer
class): after an agent-side `insert_text` op changed a block's content
MID-FOCUS, the focused GPUI editor still held its PRIOR `InputState` buffer
and the next keystroke wrote the stale buffer back over the projection — the
mechanism behind the reported "Enter at line start duplicates block". NOT
reproducible via MCP-synthesized keys (they take a clean split path); needs
a real keyboard. Same buffer-authority family as the 2026-07-10 join→stale
row (fixed only for the focus-gain edge via `converge_on_render`) and the
2026-07-20 same-seq-echo row, but a DISTINCT trigger neither fix covers: an
external write arriving WHILE the editor stays focused.

## Missing piece

The whole echo/converge composition is red-impossible headless —
`HeadlessEditorMirror::handle_keystroke` writes straight to MutableText/SQL,
bypassing `EditorViewModel`/`InputState`/`last_local_seq`/`converge_input`
(docs/Plans/EditorBufferOwnership-2026-07-20.md §0 confirms this is the
structural reason the invariant cannot go red pre-refactor). Secondary
ORACLE: no invariant "a focused editor buffer is never regressed/clobbered
by an external write landing mid-focus". Remedy = the ratified
EditorBufferOwnership refactor (move buffer+seq+echo policy into
`EditorViewModel` so the composition runs headless) + a
mid-focus-external-write rung; interim, a live-MCP twin with an
insert_text→keystroke-while-focused mix and a delta-drop fault-injection
knob.

## Remedy

OPEN — corroborated 2026-07-21; parity remedy =
EditorBufferOwnership-2026-07-20.md (ratified, in flight). Distinct from the
row-80 focus-gain `converge_on_render` fix (which does not cover an external
write mid-focus).
