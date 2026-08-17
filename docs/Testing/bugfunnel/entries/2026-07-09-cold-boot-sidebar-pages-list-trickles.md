---
id: 2026-07-09-cold-boot-sidebar-pages-list-trickles
date: 2026-07-09
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Cold-boot sidebar Pages list "trickles" in file-by-file (user report): each
  `on_file_changed` blocks up to 2s on `wait_for_blocks_in_feed` for its
  blocks to round-trip consolidator→`block_raw`→`block` matview→`LiveData`
  feed BEFORE the serial scan loop advances to the next file, so boot ≈
  N_files × (parse + write + feed barrier). At real-vault file counts this is
  seconds of gated per-page fill; latency-over-budget bug class (SLO p95
  interaction→visible <200ms)
source_line: 878
---

## Bug

Cold-boot sidebar Pages list "trickles" in file-by-file (user report): each
`on_file_changed` blocks up to 2s on `wait_for_blocks_in_feed` for its
blocks to round-trip consolidator→`block_raw`→`block` matview→`LiveData`
feed BEFORE the serial scan loop advances to the next file, so boot ≈
N_files × (parse + write + feed barrier). At real-vault file counts this is
seconds of gated per-page fill; latency-over-budget bug class (SLO p95
interaction→visible <200ms)

## Missing piece

boot ingest was excluded from the `holon_latency` target (no `boot_*`
stages) AND no benchmark boots a MANY-file vault — the keystone boots only 2
org files (`structural-page.org`+`Journals.org`), so the
serial+per-file-barrier cadence never manifests at test scale; no
boot-to-pages-complete SLO invariant exists

## Remedy

FIXED (this increment): (Option 0)
`boot_parse`/`boot_write`/`boot_feed_wait`(+`caught_up`)/`boot_place_wait`/`boot_file`/`boot_ingest_total`/`boot_feed_converge`
on the `holon_latency` target + a `BOOT INGEST` table in
`scripts/measure_latency.py`; (Option 1) initial scan buffers the per-file
feed-catch-up ids and does ONE end-of-scan `wait_for_blocks_in_feed` (30s
loud ceiling → `signal_error`) instead of N — safe because `block_raw` is
written synchronously (the per-file `get_blocks` count-check +
`ordering.children` propagation gate cover intra-file correctness; only the
sidebar-facing matview feed is deferred); scoped to initial scan (runtime
per-edit barrier byte-identical). Regression tests
`initial_scan_batched_barrier_ingests_all_files` /
`initial_scan_feed_stall_fails_loud` / `scan_flag_off_after_finish`
(`crates/holon-orgmode/tests/sync_controller_mutation_pbt.rs`); many-file
cold-boot bench via `diag_harness` `HOLON_SOAK_SEED_FILES`. Verified: 120
files/1093 blocks — 120 `boot_feed_wait` deferred to 0ms + one
`boot_feed_converge` (converged, `caught_up=true`); residual per-file cost
is now parse-bound (`boot_parse` p50 ≈ per-file dominant), not the barrier.
Open gap: no boot-to-pages-complete SLO invariant in the keystone (would
need a many-file boot rung)
