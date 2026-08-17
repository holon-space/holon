---
id: 2026-08-07-takes-1342-1394ms-interaction-visible-against
date: 2026-08-07
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  `navigate` takes 1342–1394ms interaction→visible against the 200ms SLO
source_line: 1175
---

## Bug

(overnight dogfood-explorer SECOND PASS, rebuilt at `main@origin` a5c64ba9 =
doc-boundary lock + fold-burst memo) **`navigate` takes 1342–1394ms
interaction→visible against the 200ms SLO** — p50 1364ms, p95 1391ms, max
1394ms over n=6, on a vault of 4 blocks across TWO documents. 6.8x over
budget, and the spread is only 52ms, so this is a deterministic fixed cost
paid on every document switch rather than an occasional tail. The app's own
`holon_oracles` latency-slo fired on all six (`ORACLE VIOLATION:
[latency-slo] interaction 'navigate' on block:doc-b took 1348ms
end-to-end`). Same-binary control rules out the debug profile exactly as on
the `split_block` row: `set_field` measures e2e p50 8ms / p95 12ms over
n=157 in this build. NOT attributed to the doc-boundary/fold-burst work — no
A/B against the previous tip was run, and `navigate` was not sampled in the
first pass, so this may be long-standing.

## Root cause

secondary ORACLE: overnight dogfood SECOND PASS, rebuilt at main@origin
a5c64ba9 (doc-boundary lock + fold-burst memo) — `navigate` measures p50
1364ms / p95 1391ms / max 1394ms interaction→visible (n=6) against the 200ms
SLO, on a 4-block TWO-DOCUMENT vault. 6.8x over budget, and the distribution
is very tight (1342–1394ms), so this is a deterministic fixed cost per
document switch, not a tail. The app's own latency-slo oracle fired on all
six. Same same-binary control as the split_block row rules out the debug
profile: `set_field` measures e2e p50 8ms / p95 12ms (n=157) in this build.
Attribution to the doc-boundary/fold-burst commits is NOT established — no
A/B against the previous tip was run)

## Missing piece

Same structural gap as the `split_block` latency row: the keystone is
headless and never measures the GPUI projection→paint path, and the runtime
latency-slo oracle is disclosure-only with no gate consuming it. Missing
piece = the same windowed latency gate, extended to `navigate`, plus a stage
breakdown (no `dispatch` samples were emitted for `navigate` at all, so
unlike `split_block` this one cannot yet be split into dispatch vs.
post-dispatch).

## Remedy

OPEN 2026-08-07 — diagnosis only. Evidence:
`/tmp/dogfood-2026-08-07/logs/latency-run2.txt`, `logs/app-run2.log`.
