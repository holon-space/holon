---
id: 2026-07-19-gpui-empty-block-type-here-grey
date: 2026-07-19
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  GPUI empty-block "Type here" grey placeholder is not hidden on first
  keystroke — the typed glyph draws ON TOP of the grey hint until the edit
  commits to the projection (Mac dogfood 2026-07-19, LOW/polish PERCEPTION).
  `frontends/gpui/src/render/builders/editable_text.rs` gated the placeholder
  on `has_content = !content.is_empty()`, where `content` is the COMMITTED
  `node.prop_str("content")` from the projection. The first keystroke updates
  the editor's live `InputState` immediately but does not commit `content`
  until later, so `has_content` stayed false and the absolutely-positioned
  "Type here" hint kept rendering behind the fresh text.
source_line: 1019
---

## Bug

GPUI empty-block "Type here" grey placeholder is not hidden on first
keystroke — the typed glyph draws ON TOP of the grey hint until the edit
commits to the projection (Mac dogfood 2026-07-19, LOW/polish PERCEPTION).
`frontends/gpui/src/render/builders/editable_text.rs` gated the placeholder
on `has_content = !content.is_empty()`, where `content` is the COMMITTED
`node.prop_str("content")` from the projection. The first keystroke updates
the editor's live `InputState` immediately but does not commit `content`
until later, so `has_content` stayed false and the absolutely-positioned
"Type here" hint kept rendering behind the fresh text.

## Missing piece

The headless keystone reads block content/`displayed_text` off the ViewModel
but never composites the absolutely-positioned placeholder layer over the
live InputState, so the transient overlap between typed glyph and stale
placeholder is invisible to it (pixel-composition PERCEPTION gap).

## Remedy

FIXED 2026-07-19 — placeholder visibility now keys off the LIVE editor text:
`show_placeholder = displayed_text.is_empty()` (the post-convergence
`InputState` value already snapshotted for the staleness invariants) instead
of `!has_content` (committed content). The hint disappears the instant the
first glyph lands and reappears if the block is emptied. Gate: `cargo check
-p holon-gpui` clean; live-driven on Mac (screenshot
`smu-placeholder-*.png`).
