---
id: 2026-07-28-cold-boot-logs-per-batch-info
date: 2026-07-28
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Cold-boot logs are 98 % per-batch INFO spam, blinding the diagnosis they are
  needed for: 84,143 lines from the same cold start, of which 82,611 come from
  four templates — `[Demux] view=…` 33,399 (`matview_manager.rs:392`),
  `[SqlOperationProvider] Executing batch` 16,404
  (`sql_operation_provider.rs:3243`), `[SqlOperationProvider] batch timing`
  16,404 (`:3273`), `[LoroProjection] applied N op(s)` 16,404
  (`loro_sync_controller.rs:905`). Every aggregate boot metric that would tell
  the story (`boot_ingest_total`, `boot_feed_converge`, `boot_file`) is
  `debug!` under `target: "holon_latency"`, so the default INFO level yields
  84k lines of microbatch chatter and ZERO summary.
source_line: 1117
---

## Bug

Cold-boot logs are 98 % per-batch INFO spam, blinding the diagnosis they are
needed for: 84,143 lines from the same cold start, of which 82,611 come from
four templates — `[Demux] view=…` 33,399 (`matview_manager.rs:392`),
`[SqlOperationProvider] Executing batch` 16,404
(`sql_operation_provider.rs:3243`), `[SqlOperationProvider] batch timing`
16,404 (`:3273`), `[LoroProjection] applied N op(s)` 16,404
(`loro_sync_controller.rs:905`). Every aggregate boot metric that would tell
the story (`boot_ingest_total`, `boot_feed_converge`, `boot_file`) is
`debug!` under `target: "holon_latency"`, so the default INFO level yields
84k lines of microbatch chatter and ZERO summary.

## Root cause

cold-boot logs are 98 % PER-BATCH INFO SPAM, blinding the exact diagnosis
they are needed for. The same real-vault cold start emits 84,143 lines of
which 82,611 come from just FOUR per-batch INFO templates: `[Demux] view='…'
items=N subscribers=N` 33,399 (`matview_manager.rs:392`),
`[SqlOperationProvider] Executing batch` 16,404
(`sql_operation_provider.rs:3243`), `[SqlOperationProvider] batch timing`
16,404 (`:3273`), `[LoroProjection] applied N op(s)` 16,404
(`loro_sync_controller.rs:905`). Every aggregate boot metric that WOULD tell
the story — `boot_ingest_total`, `boot_feed_converge`, `boot_file` — is
`tracing::debug!` under `target: "holon_latency"`, so at the default INFO
level a cold boot produces 84k lines of microbatch chatter and ZERO summary.
ORACLE primary: the per-batch INFO is wrong at ANY scale — a 3-block
keystone run logs INFO-per-batch too — but no invariant anywhere bounds log
lines per unit of work, so nothing has ever flagged it; ENVIRONMENT
secondary only in that the VOLUME (and hence the pain) needs vault scale to
manifest. Not a PERCEPTION gap: a line-count-per-op budget is trivially
formalizable. FIXED in this lane: all four demoted `info!`→`debug!`, and
because that would otherwise leave the INFO level with no cold-boot story at
all, TWO once-per-boot aggregate INFO lines added in their place —
`[OrgMode] initial scan complete: N file(s) in Xms, N failure(s)`
(`orgmode/src/di.rs`) and `[InitialScan] feed convergence: N block(s) in Xms
(caught_up=…)` (`file_sync_controller.rs`). Net effect on the same workload:
~82,600 INFO lines → 2. The `holon_latency` debug events and
`scripts/measure_latency.py` are untouched, so
`RUST_LOG=holon_latency=debug` still yields the full per-stage split.)

## Missing piece

No invariant anywhere bounds log lines per unit of work, so per-batch INFO
has never been flagged — and it is wrong at any scale, since a 3-block
keystone run logs INFO-per-batch too. ENVIRONMENT only in that the volume
needs vault scale to hurt. Not PERCEPTION: a lines-per-op budget is
trivially formalizable.

## Remedy

FIXED 2026-07-28 — all four demoted `info!`→`debug!`. Because that alone
would leave INFO with no cold-boot story, two once-per-boot aggregate INFO
lines replace them: `[OrgMode] initial scan complete: N file(s) in Xms, N
failure(s)` (`orgmode/src/di.rs`) and `[InitialScan] feed convergence: N
block(s) in Xms (caught_up=…)` (`file_sync_controller.rs`). Same workload:
~82,600 INFO lines → 2. `holon_latency` debug events and
`scripts/measure_latency.py` are untouched and still give the full per-stage
split in dev-profile builds. LOAD-BEARING CAVEAT found while validating: in
RELEASE builds `debug!` is compiled out entirely — the turso fork's
`workspace-hack` enables `tracing/release_max_level_info`, which
feature-unifies across the whole graph, so `RUST_LOG=holon_latency=debug` on
a release binary yields NOTHING and `measure_latency.py` has no input at
all. That predates this lane and is why the two aggregate INFO lines were
added rather than relying on the existing `boot_*` debug events: without
them a release build has ZERO cold-boot observability. Worth its own
follow-up (the vendored hakari crate silently sets the release log ceiling
for all of Holon). FOLLOW-UP DONE 2026-07-29: the `holon_latency` events are
`info!` now and gated by an EnvFilter directive instead of by their level,
so `measure_latency.py` has input from a release binary again — see the
ORACLE note on the org-scan-stall row above.
