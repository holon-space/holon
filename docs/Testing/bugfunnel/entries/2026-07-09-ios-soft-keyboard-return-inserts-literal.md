---
id: 2026-07-09-ios-soft-keyboard-return-inserts-literal
date: 2026-07-09
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  iOS soft-keyboard Return inserts a literal `\n` instead of creating a block
  (add-block dead via the on-screen keyboard). A real `enter` keystroke DOES
  split/create (verified on sim via `type_text`/`send_raw_keystroke`), so only
  the soft-keyboard insertText:→enter translation was missing
source_line: 870
---

## Bug

iOS soft-keyboard Return inserts a literal `\n` instead of creating a block
(add-block dead via the on-screen keyboard). A real `enter` keystroke DOES
split/create (verified on sim via `type_text`/`send_raw_keystroke`), so only
the soft-keyboard insertText:→enter translation was missing

## Missing piece

keystone's raw-keystroke rung injects a synthetic `KeyDown "enter"` that
bypasses gpui-mobile `handle_text_input`'s insertText: path; no
soft-keyboard-faithful (insertText:) input rung exists in the harness

## Remedy

FIXED: gpui-mobile fork `68df9dd` routes `\n`/`\r` → `enter` (mirrors the
fork's Backspace handling), pinned via Cargo.lock bump. Parity gap open: no
insertText:-path rung in keystone
