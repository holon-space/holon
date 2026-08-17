---
id: 2026-07-13-dioxus-core-trace-flood-60hz-page
date: 2026-07-13
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  dioxus-core TRACE flood at 60Hz in page console (tick pump "Marking task …
  as dirty"), drowns real errors
source_line: 985
---

## Bug

dioxus-core TRACE flood at 60Hz in page console (tick pump "Marking task …
as dirty"), drowns real errors

## Missing piece

no console-noise budget in any harness

## Remedy

FIXED (B4): page tracing layer gated at INFO
(`WASMLayerConfigBuilder::set_max_level`); `?log=trace | debug | …` URL
param opts into verbose. Console dropped from ~thousands/reload to ~0
dioxus-trace lines. NB: worker `[wasm]` stderr (tracing INFO/WARN) still
routes to `console.error` via worker-entry `printErr`, so playwright counts
them as "errors" (~86/boot) though they are benign one-shot boot logs —
cosmetic follow-up: route worker INFO to `console.log`/`console.info`
