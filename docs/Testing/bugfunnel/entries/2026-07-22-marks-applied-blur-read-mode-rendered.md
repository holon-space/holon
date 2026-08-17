---
id: 2026-07-22-marks-applied-blur-read-mode-rendered
date: 2026-07-22
gap: ORACLE
secondary: PERCEPTION
status: OPEN
summary: >-
  Marks not applied on blur — read-mode rendered_text.rs only styles Link
  marks; Bold/Italic/Underline/etc. are stored correctly (verified: content
  stripped, marks persisted, disk round-trips) but dropped to plain text by
  static_inner/build_content_segments. Editor mode styles them via
  rich_text_runs; read mode does not
source_line: 1090
---

## Bug

Marks not applied on blur — read-mode rendered_text.rs only styles Link
marks; Bold/Italic/Underline/etc. are stored correctly (verified: content
stripped, marks persisted, disk round-trips) but dropped to plain text by
static_inner/build_content_segments. Editor mode styles them via
rich_text_runs; read mode does not

## Missing piece

no read-mode styled-run assertion (windowed T3 / gpui unit) that stored
non-link marks render as styled runs; keystone asserts stored marks (green)
not rendered style

## Remedy

open (marks-render fix lane in flight 2026-07-22)
