---
id: 2026-07-20-gpui-dogfood-sqlonly-typed-trailing-whitespace
date: 2026-07-20
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  GPUI dogfood (SqlOnly): typed TRAILING whitespace is trimmed from the
  `content` projection (SQL truth) while the editor buffer keeps it — typing "
  " at end of a block leaves `content` unchanged (len frozen), but typing a
  non-space char afterward flushes the accumulated spaces (e.g. two trailing
  spaces reappear as "MORE X"). Editing during active typing is unaffected
  (mid-string spaces always preserved); the divergence is projection-only
  trailing-trim, also matching org-ingest heading trim. Low severity (likely
  intended), but a projection↔buffer divergence a user can perceive as "my
  space vanished".
source_line: 1039
---

## Bug

GPUI dogfood (SqlOnly): typed TRAILING whitespace is trimmed from the
`content` projection (SQL truth) while the editor buffer keeps it — typing "
" at end of a block leaves `content` unchanged (len frozen), but typing a
non-space char afterward flushes the accumulated spaces (e.g. two trailing
spaces reappear as "MORE X"). Editing during active typing is unaffected
(mid-string spaces always preserved); the divergence is projection-only
trailing-trim, also matching org-ingest heading trim. Low severity (likely
intended), but a projection↔buffer divergence a user can perceive as "my
space vanished".

## Missing piece

No oracle compares editor-buffer text vs projected `content` for
trailing-whitespace fidelity; if trailing-trim is intended, document it;
else preserve trailing spaces through projection. Cosmetic in SqlOnly;
verify behavior under Loro.

## Remedy

OPEN (likely by-design)
