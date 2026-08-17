---
id: 2026-07-22-clicking-mid-block-does-place-caret
date: 2026-07-22
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Clicking mid-block does not place the caret at the click position —
  read-mode click handlers (prelude.rs click_to_focus, rendered_text.rs text
  segment) ignore the MouseDownEvent position and call set_focus with no caret
  offset; editor then seeds caret to default (end/start), not the hit-tested
  offset
source_line: 1094
---

## Bug

Clicking mid-block does not place the caret at the click position —
read-mode click handlers (prelude.rs click_to_focus, rendered_text.rs text
segment) ignore the MouseDownEvent position and call set_focus with no caret
offset; editor then seeds caret to default (end/start), not the hit-tested
offset

## Missing piece

click-hit-test→caret-offset is a platform path absent headless; no windowed
rung asserting caret lands at clicked offset

## Remedy

FIXED 2026-07-22 (branch `fix-click-caret`): `rendered_text.rs` now clones
the `StyledText`/`TextLayout` before handing it to `InteractiveText` and
captures `TextLayout::index_for_position(MouseDownEvent.position)` in a
sibling parent-div `on_mouse_down`; the non-link click arm (plus the plain
mark-less `plain_caret_click` path) calls `set_focus_with_caret(block,
styled_offset_to_buffer_offset(byte))` — identity byte-domain map today
(styled text == stripped buffer content), the single seam raw-edit I2 swaps
for the `RawOffsetMap`. Click past the glyphs (`index_for_position` Err)
falls back to plain `set_focus` (caret defaults to end) — disclosed
degradation. COVERAGE gap closed by new windowed rung
`frontends/gpui/tests/layout_editor.rs::windowed_caret` (click-at-char-x →
`TestServices::recorded_caret` asserts the seeded offset, incl. UTF-8
byte-offset + past-end fallback cases).
