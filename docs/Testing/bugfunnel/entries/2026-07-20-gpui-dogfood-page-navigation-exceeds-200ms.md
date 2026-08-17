---
id: 2026-07-20-gpui-dogfood-page-navigation-exceeds-200ms
date: 2026-07-20
gap: ORACLE
secondary: ENVIRONMENT
status: UNCLASSIFIED
summary: >-
  GPUI dogfood: page navigation exceeds the 200ms interaction→visible SLO at
  trivial scale. Live "ORACLE VIOLATIONS — live invariant check failed" banner
  fired with `[latency-slo] interaction 'navigate' … took 685ms` and `…
  501ms`; `measure_latency.py` confirms navigate p50=593ms / p95=676ms /
  max=685ms over 2 samples, on a ~6-page seeded vault. `set_field` (typing) is
  healthy (p50=18ms, p95=46ms) — the cost is navigation-specific (recursive
  `focus_descendants` matview re-run + reproject on each nav).
source_line: 1034
---

## Bug

GPUI dogfood: page navigation exceeds the 200ms interaction→visible SLO at
trivial scale. Live "ORACLE VIOLATIONS — live invariant check failed" banner
fired with `[latency-slo] interaction 'navigate' … took 685ms` and `…
501ms`; `measure_latency.py` confirms navigate p50=593ms / p95=676ms /
max=685ms over 2 samples, on a ~6-page seeded vault. `set_field` (typing) is
healthy (p50=18ms, p95=46ms) — the cost is navigation-specific (recursive
`focus_descendants` matview re-run + reproject on each nav).

## Missing piece

The live prod banner is the only oracle that caught this; the headless
keystone has no navigate e2e-latency invariant (only `set_field` emits
`e2e`; `navigate` latency is unasserted in the composed harness). Add a
navigate-latency invariant/budget to the keystone; investigate the
focus-descendants recompute cost.

## Remedy

RE-TRIAGED 2026-07-20 (same day): MEASUREMENT ARTIFACT, not a nav dominator
— 79-91% of both measured windows was MCP-driver transport idle (gap sizes
match the rmcp/serve_inner request cadence ~95/143ms, incl. unrelated
agent-injected clicks inside the window); backend compute ~25ms; no per-nav
DDL; watch views reused; RC5 (91ms) NOT regressed; not scale-dependent. SLO
verdict deferred to clean re-measurement (release build, idle machine, REAL
clicks, N>=10 cold navs) — protocol + full phase tables in orchestrator
scratch nav-latency/ANALYSIS.md. Instrumentation gap noted: no stage between
navigation.focus dispatch and render_entity/query_and_watch.
