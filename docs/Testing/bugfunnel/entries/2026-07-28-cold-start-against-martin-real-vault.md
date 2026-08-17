---
id: 2026-07-28-cold-start-against-martin-real-vault
date: 2026-07-28
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Cold start against Martin's real vault (1,001 org files / 25,168 blocks)
  keeps the "org initial scan" readiness gate CLOSED for 1,472 s (24.5 min)
  while the GPUI window reports "Session ready" at t+2.9 s — the app is
  interactive-but-empty for 24 minutes. The org scan writes ONE BLOCK PER LORO
  COMMIT and `LoroSyncController`'s wake loop keeps up, so
  `LoroProjection::project()` runs 16,451 times (87.2 % of passes emit exactly
  1 op). Two costs follow from that cadence and are 61 % of the boot: (1)
  sibling-scope re-read amplification — `incremental_block_changes` expands
  every dirty scope to its FULL child list to recompute tie-break keys, so the
  k-th block of a page re-reads k siblings and `blocks_differ` then discards
  nearly all of them (O(K²) per page); pre-emit `snapshot_ms` climbs 0.9 ms
  mean at 10k blocks → 266 ms at 20–22k, 567 s total = 38.5 % of boot. (2)
  16,451 single-op SQL transactions carrying 25,765 statements, mean 20.4 ms
  each = 335 s = 22.7 % of boot, dominated by transaction overhead not
  statement work.
source_line: 1116
---

## Bug

Cold start against Martin's real vault (1,001 org files / 25,168 blocks)
keeps the "org initial scan" readiness gate CLOSED for 1,472 s (24.5 min)
while the GPUI window reports "Session ready" at t+2.9 s — the app is
interactive-but-empty for 24 minutes. The org scan writes ONE BLOCK PER LORO
COMMIT and `LoroSyncController`'s wake loop keeps up, so
`LoroProjection::project()` runs 16,451 times (87.2 % of passes emit exactly
1 op). Two costs follow from that cadence and are 61 % of the boot: (1)
sibling-scope re-read amplification — `incremental_block_changes` expands
every dirty scope to its FULL child list to recompute tie-break keys, so the
k-th block of a page re-reads k siblings and `blocks_differ` then discards
nearly all of them (O(K²) per page); pre-emit `snapshot_ms` climbs 0.9 ms
mean at 10k blocks → 266 ms at 20–22k, 567 s total = 38.5 % of boot. (2)
16,451 single-op SQL transactions carrying 25,765 statements, mean 20.4 ms
each = 335 s = 22.7 % of boot, dominated by transaction overhead not
statement work.

## Root cause

cold start against Martin's real vault (1,001 org files) keeps the "org
initial scan" readiness gate CLOSED for 1,472 s — 24.5 minutes, not the 60 s
the first look suggested. Measured from the dogfood log
`/private/tmp/holon-cold.log`: boot at `16:07:19.94`, `[post_ready] org scan
complete — sync gate opened` at `16:31:52.68`; the GPUI window says "Session
ready" at t+2.9 s, so the app is INTERACTIVE-BUT-EMPTY for 24 minutes while
`mcp_integration` logs `sync still deferred — org initial scan in progress`
all the way to `waited_s=221` and beyond. Root cause is the projection
CADENCE, not any single slow stage: the org scan writes ONE BLOCK PER LORO
COMMIT, each commit notifies `LoroSyncController`'s wake loop
(`loro_sync_controller.rs:274`), and the loop keeps up — so
`LoroProjection::project()` runs 16,451 times for a 25,168-block vault (87.2
% of those passes emit exactly ONE op; mean 1.54 ops, median 1). Two costs
fall out of that cadence and together account for 61 % of the whole boot.
(1) SIBLING-SCOPE RE-READ AMPLIFICATION — 567 s (38.5 % of boot). The
"O(changed)" incremental path is not O(changed): `incremental_block_changes`
(`loro_backend.rs:1482-1486`) expands every dirty scope to its FULL child
list, because a structural change can shift the sibling tie-break key of
every current member. Ingesting the k-th block of a page therefore re-reads
all k siblings, then `blocks_differ` compare-and-skip discards nearly all of
them — O(K²) per page, paid 16,451 times. The pre-emit cost (`snapshot_ms`)
climbs from 0.9 ms mean while the doc is at 10k blocks to 266 ms mean at
20–22k. (2) PER-OP SQL TRANSACTIONS — 335 s (22.7 % of boot). 16,451
separate `db_handle.transaction()` calls carrying 25,765 statements total,
mean 20.4 ms / median 8 ms of transaction time for 1.5 ops of work;
transaction overhead, not statement work, dominates. ENVIRONMENT primary per
the rubric's "real-vault scale" clause and its explicit latency rule:
nothing about the interaction is missing — a cold boot is the most ordinary
thing the app does — but the failing regime needs ~1,000 files / ~25k
blocks, and the keystone's scale knob `HOLON_SOAK_SEED_BLOCKS` defaults to
0, so no default run has ever been within two orders of magnitude. ORACLE
secondary and durable: there is NO boot-readiness budget invariant, so a
soak-seeded run would sit for 24 minutes and still report GREEN. DISTINCT
FROM the vault-scale `Delta::consolidate` wedge two rows below though found
on the same vault: that one is Turso-side unbounded commit cost per write;
this one is Holon-side cadence — we hand Turso and Loro 16,451 microbatches
when ~1,000 per-file batches would carry the same work. Fix candidates
ranked in the lane report; both collapse to "batch the initial scan per file
instead of per block", which the existing
`begin_initial_scan`/`finish_initial_scan` scan-mode bracket
(`file_sync_controller.rs:455`) already provides the seam for. STATUS
2026-07-29 — the Holon-side cadence half is FIXED (see the boot-projector
gating row above): the per-file batching this row asked for already existed,
and the 16,451 microbatches were the projector run loop reconciling per Loro
commit during the scan, not the scan itself. The
`begin_initial_scan`/`finish_initial_scan` seam was never the problem, so no
architectural batching change was needed. Two halves remain OPEN and are
tracked elsewhere: (a) the ORACLE gap named above — there is still no
boot-readiness budget invariant, so a soak-seeded run that sits for minutes
still reports GREEN; (b) the Turso-side vault-scale `Delta::consolidate`
commit cost, which is the row two below, not this one. The 1,472 s figure
itself has NOT been re-measured against Martin's vault post-fix.)

## Missing piece

The failing regime needs ~1,000 files / ~25k blocks; the keystone's scale
knob `HOLON_SOAK_SEED_BLOCKS` defaults to 0, so no default run is within two
orders of magnitude of it. Nothing about the interaction is missing — a cold
boot is the most ordinary thing the app does. ORACLE secondary and durable:
there is NO boot-readiness budget invariant, so a soak-seeded run would sit
for 24 minutes and still report GREEN.

## Remedy

OPEN 2026-07-28 — measured and root-caused, NOT fixed. Evidence:
`/private/tmp/holon-cold.log` (read-only reference), per-batch stats in the
lane report. Fix candidates ranked there; all collapse to "batch the initial
scan per file instead of per block", for which the existing
`begin_initial_scan`/`finish_initial_scan` bracket
(`file_sync_controller.rs:455`) is the seam. Deliberately deferred: the
batching change is architectural and needs a red-first keystone at soak
scale per the `holon-feature` contract. DISTINCT from the vault-scale
`Delta::consolidate` wedge row (Turso-side unbounded per-write commit cost)
though found on the same vault — this row is Holon-side cadence.
