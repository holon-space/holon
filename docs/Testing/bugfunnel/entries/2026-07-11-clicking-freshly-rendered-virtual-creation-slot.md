---
id: 2026-07-11-clicking-freshly-rendered-virtual-creation-slot
date: 2026-07-11
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  Clicking a freshly-rendered virtual creation-slot entity fails ('no bounds
  recorded ... BoundsRegistry hasn't promoted staged→committed'); the failed
  click silently leaves focus on a stale block with the position-0 caret, so
  typed text + Enter SPLIT AN UNRELATED EXISTING BLOCK instead of creating one
  — no error surfaced to the user (fail-loud violation)
source_line: 895
---

## Bug

Clicking a freshly-rendered virtual creation-slot entity fails ('no bounds
recorded ... BoundsRegistry hasn't promoted staged→committed'); the failed
click silently leaves focus on a stale block with the position-0 caret, so
typed text + Enter SPLIT AN UNRELATED EXISTING BLOCK instead of creating one
— no error surfaced to the user (fail-loud violation)

## Missing piece

bounds-registry staged→committed timing race on fresh elements absent from
test env; click errors reaching the input layer have no loud surface

## Remedy

OPEN
