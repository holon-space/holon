---
id: 2026-08-02-cold-boot-copy-real-vault-1001
date: 2026-08-02
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Cold boot on a copy of the real vault (1001 org files) spends 8m22s in the
  initial org scan — `stage=boot_ingest_total ms=501506`, per-file `boot_file`
  p50 1.8s / p95 16.8s / max 75.7s, `boot_place_wait` p95 21.3s / max 213.7s
  (`scripts/measure_latency.py` over the session log). For that entire window
  the MCP integration sync gate is held closed by design (`await_gate`,
  `crates/holon-mcp-client/src/mcp_integration.rs:1208-1243`, which correctly
  WARNs `sync still deferred ... waited_s=420`), so every integration cache
  table is EMPTY and every integration-backed page renders blank — with no
  banner, per the DegradedSignalBus row above. Orders of magnitude over the
  200ms interaction SLO for the first-usable-state measure a user actually
  experiences.
source_line: 1145
---

## Bug

(dogfood, ClaudeCode.org build-out on a copy of the real vault, port 8710)
Cold boot on a copy of the real vault (1001 org files) spends 8m22s in the
initial org scan — `stage=boot_ingest_total ms=501506`, per-file `boot_file`
p50 1.8s / p95 16.8s / max 75.7s, `boot_place_wait` p95 21.3s / max 213.7s
(`scripts/measure_latency.py` over the session log). For that entire window
the MCP integration sync gate is held closed by design (`await_gate`,
`crates/holon-mcp-client/src/mcp_integration.rs:1208-1243`, which correctly
WARNs `sync still deferred ... waited_s=420`), so every integration cache
table is EMPTY and every integration-backed page renders blank — with no
banner, per the DegradedSignalBus row above. Orders of magnitude over the
200ms interaction SLO for the first-usable-state measure a user actually
experiences.

## Missing piece

The keystone's vaults are tens of generated files, so per-file boot cost and
the gate's hold time never reach a scale where either is observable; no test
asserts a bound on `boot_ingest_total` or on time-to-first-integration-row.
Missing piece = a scale/latency gate over a synthetic 1000-file vault with a
budget on both.

## Remedy

OPEN — diagnosis only. Related to the previously-landed cold-boot work but
measured here on the ORG scan rather than the commit log.
