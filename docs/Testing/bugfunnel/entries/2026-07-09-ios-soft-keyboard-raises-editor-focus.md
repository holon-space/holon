---
id: 2026-07-09-ios-soft-keyboard-raises-editor-focus
date: 2026-07-09
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  iOS soft keyboard raises on editor focus then hides (~150ms,
  `KEYBOARD_HIDE_GRACE` / focus churn) — appears then dismisses rather than
  staying visible
source_line: 871
---

## Bug

iOS soft keyboard raises on editor focus then hides (~150ms,
`KEYBOARD_HIDE_GRACE` / focus churn) — appears then dismisses rather than
staying visible

## Missing piece

keyboard show/hide is a device-visual timing property; no headless
assertion; render-edge `editor_focus_gained/lost` + deferred-hide grace race

## Remedy

open
