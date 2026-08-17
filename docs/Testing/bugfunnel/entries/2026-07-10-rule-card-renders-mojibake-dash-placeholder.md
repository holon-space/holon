---
id: 2026-07-10-rule-card-renders-mojibake-dash-placeholder
date: 2026-07-10
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  Rule card renders `last fired: â` — mojibake (em-dash placeholder mangled)
  in live GPUI render
source_line: 841
---

## Bug

Rule card renders `last fired: â` — mojibake (em-dash placeholder mangled)
in live GPUI render

## Missing piece

no visual/encoding assertion possible in headless harness

## Remedy

FIXED (ASCII placeholders + `rule_card_render_has_no_mojibake_bytes` test).
Root cause is DOWNSTREAM: DSL parse preserves multibyte (proven by probe);
the em-dash's first UTF-8 byte leaked on the live GPUI render/capture path —
see separate PERCEPTION row below
