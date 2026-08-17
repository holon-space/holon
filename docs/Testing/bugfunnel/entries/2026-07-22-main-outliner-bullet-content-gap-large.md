---
id: 2026-07-22-main-outliner-bullet-content-gap-large
date: 2026-07-22
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  Main-outliner bullet-to-content gap large: block row stacks a 20px orgmode ◉
  bullet box + a 20px state_toggle box (empty for non-task blocks) + spacer +
  rendered_text's own 12px left padding, so text starts ~50px in while the
  visible dot is 16px (style.rs icon_size=16/box_padding=4; "Option A
  12px/16px" not present on this main)
source_line: 1095
---

## Bug

Main-outliner bullet-to-content gap large: block row stacks a 20px orgmode ◉
bullet box + a 20px state_toggle box (empty for non-task blocks) + spacer +
rendered_text's own 12px left padding, so text starts ~50px in while the
visible dot is 16px (style.rs icon_size=16/box_padding=4; "Option A
12px/16px" not present on this main)

## Missing piece

no layout snapshot on bullet-to-text offset; double icon-box gutter + text
px(12) uncollapsed

## Remedy

open
