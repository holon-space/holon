---
id: 2026-08-04-cold-boot-ingest-takes-237-seconds
date: 2026-08-04
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Cold-boot ingest takes 237 seconds and is entirely unmeasured by any
  automated layer.
source_line: 782
---

## Bug

(dogfood, real-vault scale, cold boot on a 182 MB / 139-file copy of the
real vault) **Cold-boot ingest takes 237 seconds and is entirely unmeasured
by any automated layer.** `scripts/measure_latency.py` on the boot log:
`boot_ingest_total` 237449 ms (one sample); `boot_file` n=138, p50 431 ms,
**p95 7360 ms, max 28494 ms** for a SINGLE file; `boot_write` n=102 p95 5847
ms max 19453 ms; `boot_place_wait` n=139 p95 3573 ms max 12149 ms;
`boot_parse` n=139 p95 629 ms; `matview_ddl` n=127 p95 70 ms max 3987 ms.
The app is unusable for ~4 minutes after launch. Distinct from the
previously-closed cold-boot row (the one-file Turso O(N²) cursor, fixed
2026-07-29): here the cost is spread across many files, dominated by
`boot_file`/`boot_write`, and the tail (28 s for one file) suggests a
per-file cost that grows with something the fixtures never grow.

## Missing piece

Every fixture vault is small, so no automated layer ever boots at real-vault
scale and there is no budget invariant on boot at all. Missing piece = (i) a
scale fixture (or a synthetic vault generator sized to the real one) driven
in a nightly, and (ii) a boot-time budget the way interaction latency has
one, so a regression in per-file ingest cost is caught by a number rather
than by Martin waiting.

## Remedy

OPEN 2026-08-04 — diagnosis only, no profiling done in this lane (the 28 s
outlier file was not identified).
