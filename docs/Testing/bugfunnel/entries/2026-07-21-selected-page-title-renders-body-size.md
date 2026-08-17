---
id: 2026-07-21-selected-page-title-renders-body-size
date: 2026-07-21
gap: PERCEPTION
secondary: COVERAGE
status: FIXED
summary: >-
  Selected page/title renders at body size (14px), not a heading —
  block_profile.yaml:54 page_title asks style:"h1" but the text shadow-builder
  (shadow_builders/text.rs:7) has no style param and SILENTLY DROPS the kwarg
  → 14.0 default. Sub-bug (fail-loud violation): shadow-builders silently
  ignore unknown widget kwargs.
source_line: 1052
---

## Bug

Selected page/title renders at body size (14px), not a heading —
block_profile.yaml:54 page_title asks style:"h1" but the text shadow-builder
(shadow_builders/text.rs:7) has no style param and SILENTLY DROPS the kwarg
→ 14.0 default. Sub-bug (fail-loud violation): shadow-builders silently
ignore unknown widget kwargs.

## Missing piece

widget-tree invariant: page-title text size >= heading threshold; hard-fail
(or loud warn) on unknown widget kwargs instead of silent drop

## Remedy

FIXED+WOVEN 2026-07-21 (cycle 2) — style declared param + render-time
resolution (h1=28) at the CDC-fast-path-shared choke point; unknown style
values warn loud. Verifier CONFIRMED; round-4 live-confirmed (28px vs 15px)
