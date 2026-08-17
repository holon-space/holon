---
id: 2026-07-13-error-empty-state-unstyled-dev-text
date: 2026-07-13
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  Error/empty state was unstyled dev text, no recovery action
source_line: 995
---

## Bug

Error/empty state was unstyled dev text, no recovery action

## Missing piece

none

## Remedy

FIXED (B2/B3): boot-failed + no-root-layout states now render a centered
recovery card (muted danger palette on the app's dark theme) with a clear
message + "Reset local data" action
