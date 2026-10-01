---
id: 2026-10-01-slash-commands-are-never-generated-on-the-dispatch-editor-leg
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  TriggerSlashCommand is deselected on both dispatch editor-leg points
  ({Loro + dispatch} and {Turso + dispatch}), so the slash-command path GPUI
  runs in production has no headless coverage.
---

## Bug
Found by the verifier of the editor-leg-axis lane (`lane-logs/adm0b-verify.md`, O4 and evidence
item 5). GPUI commits through the dispatch leg (`frontends/gpui/src/di.rs` installs no cell
registry). The keystone can now boot that leg headless, but it never generates a slash command on
it.

## Root cause
`crates/holon-integration-tests/src/pbt/transitions/trigger_slash_command.rs` gates on
`editor_cell_attached()` (`Reason::EditorCellRequired`): its driver waits on the cell's
`editable_text` (`MutableText`), which never resolves on a dispatch leg. Before the leg was a
drawn axis, {Loro + dispatch} was never booted headless, so the gap was hidden rather than new.

## Missing piece
A slash-command driver that works on the dispatch leg (keystrokes through the dispatcher, as GPUI
sends them) instead of through the editor cell.

## Remedy
Open. A slash row that must hold on all three editor-leg points cannot be written headless until
that driver exists.
