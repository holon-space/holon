---
id: 2026-07-11-suite-gated-default-compiles-empty-tests
date: 2026-07-11
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  `sync_controller_mutation_pbt` suite is `#![cfg(feature = "di")]`-gated —
  the default `cargo nextest run -p holon-orgmode` compiles it EMPTY (0
  tests), so the suite rotted invisibly through prod redesigns and a REAL bug
  hid in it: org ingest never emitted the `advice_suppressed` typed edge array
  (ADR 0021) — `:ADVICE_SUPPRESSED:` drawers never reached the junction + a
  stray string key polluted properties (round-trip data loss for suppression
  edges)
source_line: 892
---

## Bug

`sync_controller_mutation_pbt` suite is `#![cfg(feature = "di")]`-gated —
the default `cargo nextest run -p holon-orgmode` compiles it EMPTY (0
tests), so the suite rotted invisibly through prod redesigns and a REAL bug
hid in it: org ingest never emitted the `advice_suppressed` typed edge array
(ADR 0021) — `:ADVICE_SUPPRESSED:` drawers never reached the junction + a
stray string key polluted properties (round-trip data loss for suppression
edges)

## Missing piece

no CI matrix entry runs feature-gated test binaries; nothing asserts "every
test binary runs somewhere"

## Remedy

FIXED (test-triage stream): prod emit + drawer-key skip (mirrors
`requires`); 4 stale-harness failures re-aligned to prod semantics (`#+ID:`
injection, tags-as-Array, positional-param alias, merge-aware upsert); suite
42/42 with `--features di`. OPEN gap: the CI-matrix/feature-gate audit
itself
