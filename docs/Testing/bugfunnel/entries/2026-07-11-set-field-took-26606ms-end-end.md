---
id: 2026-07-11-set-field-took-26606ms-end-end
date: 2026-07-11
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  set_field took 26606ms end-to-end (SLO p95<200ms, 133x over) during rapid
  split/join/undo/redo/type churn on ONE block at fresh-boot scale —
  self-detected by the app's own latency oracle
source_line: 897
---

## Bug

set_field took 26606ms end-to-end (SLO p95<200ms, 133x over) during rapid
split/join/undo/redo/type churn on ONE block at fresh-boot scale —
self-detected by the app's own latency oracle

## Missing piece

keystone generates no rapid same-block structural-op churn; oracle exists,
corpus doesn't

## Remedy

OPEN
