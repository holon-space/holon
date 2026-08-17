---
id: 2026-07-13-slo-banner-10000ms-root-caused-phantom
date: 2026-07-13
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  SLO banner ">10000ms" root-caused as PHANTOM measurement (Martin live report
  #2): `latency_e2e` correlator (crates/holon-api/src/latency_e2e.rs) matches
  a delivered CDC batch against the OLDEST pending entry for the target id; a
  dispatch that produces NO row change (e.g. the spurious identical-content
  blur commit above, or any coalesced/no-op write) leaves a stale pending
  entry (30s expiry) which STEALS the match from the next real commit on the
  same block → observed live: pair `e2e ms=16592` + `ms=10` emitted in the
  same millisecond at blur, ORACLE VIOLATION banner fired on an 18-block vault
  whose real latency was 10ms. At 800-block vault scale REAL latency is
  healthy: set_field e2e p50=14.5ms p95=32.6ms max=36ms (n=18), zero real SLO
  breaches, zero quarantine — the >10s banners Martin sees are correlator
  artifacts, not pipeline stalls
source_line: 973
---

## Bug

SLO banner ">10000ms" root-caused as PHANTOM measurement (Martin live report
#2): `latency_e2e` correlator (crates/holon-api/src/latency_e2e.rs) matches
a delivered CDC batch against the OLDEST pending entry for the target id; a
dispatch that produces NO row change (e.g. the spurious identical-content
blur commit above, or any coalesced/no-op write) leaves a stale pending
entry (30s expiry) which STEALS the match from the next real commit on the
same block → observed live: pair `e2e ms=16592` + `ms=10` emitted in the
same millisecond at blur, ORACLE VIOLATION banner fired on an 18-block vault
whose real latency was 10ms. At 800-block vault scale REAL latency is
healthy: set_field e2e p50=14.5ms p95=32.6ms max=36ms (n=18), zero real SLO
breaches, zero quarantine — the >10s banners Martin sees are correlator
artifacts, not pipeline stalls

## Missing piece

the latency ORACLE fires on a mis-attributed measurement; correlator has no
dispatch-identity (op-instance) correlation, only target-id FIFO; no-op
dispatches are never closed

## Remedy

OPEN — fix direction: correlate by op-instance token (or close/skip entries
whose dispatch produced no CDC delta), and make the SLO oracle ignore
entries older than the previous batch on the same target
