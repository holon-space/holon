---
id: 2026-07-13-header-showed-green-ready-nms-while
date: 2026-07-13
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Header showed green "ready (Nms)" while projection failed to render —
  cosmetic success over failed state
source_line: 988
---

## Bug

Header showed green "ready (Nms)" while projection failed to render —
cosmetic success over failed state

## Missing piece

no invariant ties "ready" status to a non-empty projection

## Remedy

FIXED (B3): `BootState::Ready` is now flipped only on the FIRST successful
root-layout watch envelope (not eagerly after subscribing); a watchdog fails
loud (`Failed`) if no projection arrives within 10s; root-layout-absent
routes to `NoRootLayout`, not green. Verified: corrupt vault shows "⚠ local
data corrupt", never green
