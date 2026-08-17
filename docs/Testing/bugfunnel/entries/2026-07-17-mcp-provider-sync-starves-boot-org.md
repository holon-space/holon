---
id: 2026-07-17-mcp-provider-sync-starves-boot-org
date: 2026-07-17
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  MCP provider sync starves the boot org scan on the serialized DatabaseActor:
  during GPUI boot the `claude-history` integration fired 79 "resource
  updated, re-sync" notifications (claude-history://projects 47×, tasks 16×,
  sessions 16×) — the FDW source is live Claude Code session data written
  continuously, so the subscription fires constantly — and EACH ran a FULL
  `sync_entity` through the single serialized DatabaseActor, interleaving with
  per-file org ingest writes; org boot ingest saw per-file stalls of 6–25s
  (~57s of the worst file's 59.7s window straddling sync ops). No debounce, no
  boot-ordering: every notification = one full re-sync, all concurrent with
  the scan
source_line: 813
---

## Bug

MCP provider sync starves the boot org scan on the serialized DatabaseActor:
during GPUI boot the `claude-history` integration fired 79 "resource
updated, re-sync" notifications (claude-history://projects 47×, tasks 16×,
sessions 16×) — the FDW source is live Claude Code session data written
continuously, so the subscription fires constantly — and EACH ran a FULL
`sync_entity` through the single serialized DatabaseActor, interleaving with
per-file org ingest writes; org boot ingest saw per-file stalls of 6–25s
(~57s of the worst file's 59.7s window straddling sync ops). No debounce, no
boot-ordering: every notification = one full re-sync, all concurrent with
the scan

## Missing piece

the ONE keystone boot rung boots a tiny 2-file vault with NO concurrent MCP
provider load — nothing in the harness runs a live/faked MCP resource-update
storm against a DatabaseActor that is simultaneously ingesting a many-file
vault, so neither the contention nor the missing debounce/gate is
observable; needs a many-file cold-boot soak rung (HOLON_SOAK_SEED_FILES
exists) crossed with a fake-MCP notification storm asserting (a) zero
provider `sync_entity` before scan-complete and (b) bounded
re-syncs/resource

## Remedy

FIXED 2026-07-17 (two composing levers, holon-mcp-client + holon-core +
holon-app). LEVER 1 DEFER: new `SyncGate` newtype
(`holon-core/src/sync_gate.rs`, `enum SyncGateState { DeferredUntilScan,
Open }` — parse-don't-validate, no bool) registered as a DI singleton in
`wiring.rs::add_frontend`, opened by the `post_ready` org-scan barrier on
EVERY completion path (success / per-file degraded / stall-error /
no-org-module) so a deferred sync always eventually runs.
`spawn_sync_event_loop` awaits the gate before ANY sync (initial +
notification + poll); signals that arrive during the scan buffer in the
unbounded channel and coalesce. Fail-loud: while deferred it re-WARNs every
60s (visible, never silent), and an absolute 600s watchdog proceeds in
DISCLOSED-degraded mode if the gate never opens (sized above realistic scan
wall). LEVER 2 DEBOUNCE/COALESCE: the loop drains via a per-resource
trailing-edge (2s) + max-wait (10s) debounce (`PendingSyncWork` dedupes by
URI/entity; a pending `SyncAll` subsumes per-URI resyncs) so 47 rapid
signals for one URI collapse to ONE re-sync. Repro-first tests
(`sync_loop_gate_debounce_tests` in mcp_integration.rs, over a counting
`ResyncSink` fake — new trait extracted so the loop is unit-testable with no
live peer): `nothing_runs_before_scan_complete_then_one_coalesced_sync`
(47×2 storm during closed gate → 0 executions, then 1 SyncAll),
`debounce_collapses_per_resource` (79 signals / 3 URIs → 3 resyncs),
`watchdog_runs_sync_if_gate_never_opens`. FLAGGED not fixed: incremental
`sync_entity` (each re-sync is still a full diff — the larger fix). Keystone
soak-rung gap remains OPEN (this is the COVERAGE secondary).
