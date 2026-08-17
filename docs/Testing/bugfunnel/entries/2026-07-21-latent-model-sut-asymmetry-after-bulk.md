---
id: 2026-07-21-latent-model-sut-asymmetry-after-bulk
date: 2026-07-21
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  Latent model/SUT asymmetry: after bulk-external org rewrite the SUT shows
  journal date-pages with EMPTY tags (is_page=false) while the model's seed
  path tags them Page — a page-ness-sensitive op (e.g. D1 outdent guard) could
  surface the disagreement as tree divergence (proven NOT the cause of
  tonight's reds, but real).
source_line: 1063
---

## Bug

Latent model/SUT asymmetry: after bulk-external org rewrite the SUT shows
journal date-pages with EMPTY tags (is_page=false) while the model's seed
path tags them Page — a page-ness-sensitive op (e.g. D1 outdent guard) could
surface the disagreement as tree divergence (proven NOT the cause of
tonight's reds, but real).

## Missing piece

decide correct date-page tag semantics on bulk-external rewrite; align
model<->SUT; correspondence assertion on date-page tags after external
rewrite

## Remedy

OPEN (W7 item-3 investigation; latent, pre-existing)
