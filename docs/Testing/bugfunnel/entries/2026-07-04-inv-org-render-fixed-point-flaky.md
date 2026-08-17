---
id: 2026-07-04-inv-org-render-fixed-point-flaky
date: 2026-07-04
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  inv-org-render-fixed-point flaky (silent 150ms settle budget)
source_line: 865
---

## Bug

inv-org-render-fixed-point flaky (silent 150ms settle budget)

## Missing piece

settle budget « projection pass; silent timeout

## Remedy

FIXED (fail-loud 30s combined settle)
