---
id: 2026-10-03-ingest-watchdog-reports-a-stall-during-the-downstream-flush
date: 2026-10-03
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The intra-file ingest watchdog warns "2001 of 2001 block(s) done ... this one file's ingest is
  stalled" while the file's downstream flush still runs a Loro full reprojection that finishes.
---

## Bug

A bulk ingest of one 2001-block org file logs, one or more times:

```
WARN holon_filesystem::ingest_progress: [Ingest] NO PROGRESS for 30s inside a SINGLE file:
  2001 of 2001 block(s) done in .../dense_page_0.org — this one file's ingest is stalled
```

The ingest is not stalled. In every measured run it finishes ("done: 2001 block(s) in
31-55 s"). Found by the verifier of the reds lane in a full core nextest run
(`scratchpad/night/verify-reds/full-core.log:4872,4874`), then measured by the reds-fix-5 lane
(`main-reds/lane-logs/reds-fix-5/`). Test profile only, no release run.

## Root cause

The watchdog counts only the per-block loop ticks (`IngestProgress::advance`,
`crates/holon-filesystem/src/ingest_progress.rs:154`). It warns when that counter does not move
for `INTRA_FILE_STALL` = 30 s (:104-135). After the last tick of the place phase, the ingest
still runs `flush_downstream` (`crates/holon-filesystem/src/file_sync_controller.rs:6527`). For
2001 new blocks the Loro projection takes a full walk (`reason=oversized`,
`crates/holon-loro/src/loro_sync_controller.rs:1340-1349`) and applies all 2001 ops in one
`consolidator.apply` call (:1725). No counter moves during that call. Measured walk times
(`stage="projection" ops=2001 mode="full"`):

| load (1 min) | walk | ingest total | log |
|---|---|---|---|
| ~22 | 18.7 s | 31.1 s | `iso-baseline-1.clean` |
| ~55-60 | 22.7-33.9 s | 37.6-54.6 s | `load-baseline-{1,2}.clean` |
| ~130 | 25.7-28.5 s | 39.5-44.8 s | `probe-wide-{1,2}.clean` |

After the walk, 5-10 s more pass before the file's `done` line. Under load the sum passes 30 s,
and the watchdog reports a stall for work that is running.

## Missing piece

No test checks the watchdog against a file whose work continues after its last block. The unit
tests cover only "no block lands" and "blocks keep landing"
(`crates/holon-filesystem/src/ingest_progress.rs:228,245`). No invariant fails on a false
stall warning.

## Remedy

OPEN. Product code is not changed. Two options:
- The downstream flush and the Loro full walk tick a liveness counter that the watchdog reads.
  The composed bulk settle (`wide_e2e::settle_while_progressing`) could read the same counter
  and use a much shorter stall window than its current 120 s.
- The watchdog names the phase it waits in ("downstream flush"), so the warning is true.
