---
id: 2026-09-19-boot-ingest-of-the-real-vault-takes-thirty-nine-seconds
date: 2026-09-19
gap: ENVIRONMENT
status: OPEN
summary: >-
  Initial scan of a 1031-file real vault took 39.5s, with one single file
  costing 16.9s of it and a `matview_ddl` stage costing 9.3s, all of it in front
  of the first usable frame.
---

## Bug

Found by the `dogfood-integ` lane booting the real GPUI binary on a copy of
Martin's vault. Log: `lane-logs/dogfood-integ-evidence/logs/app-boot1.log`,
measured by `scripts/measure_latency.py` into
`lane-logs/dogfood-integ-latency-boot1.log`.

| stage | n | p50 | p95 | max |
|---|---|---|---|---|
| boot_ingest_total | 1 | — | — | 39496 ms |
| boot_file | 131 | 0 ms | 185 ms | 16859 ms |
| boot_parse | 14 | 170 ms | 3013 ms | 7071 ms |
| boot_write | 2 | — | — | 8038 ms |
| matview_ddl | 104 | 0 ms | 183 ms | 9283 ms |

The runtime oracle flagged eight of these individually, e.g.

```
[latency-slo diagnostic] 'boot_ingest_total' stage took 39496ms (> 200ms budget)
[latency-slo diagnostic] 'boot_file' stage took 16859ms (> 200ms budget)
[latency-slo diagnostic] 'matview_ddl' stage took 9283ms (> 200ms budget)
```

The second boot ingested in 17190 ms, so roughly half of the first figure is
cold-cache cost that does not recur; the remaining ~17s does.

The shape is not uniform slowness. `boot_file` p50 is 0 ms over 131 files and
max is 16859 ms — a long tail of a handful of files, not a flat per-file cost.
The same asymmetry holds for `matview_ddl` (p50 0 ms, max 9283 ms).

## Root cause

Partly isolated by lane `nav-latency-rca` (2026-09-19); report
`lane-logs/nav-latency-rca-report.md`.

Reproduced on a fresh copy of the same vault: 55397 ms with integrations and
60791 ms without, so integration sync is not the boot cost. Measurement files
`lane-logs/nav-latency-run1-A-realvault-with-integrations.log` and
`lane-logs/nav-latency-run2-B-realvault-no-integrations.log`. The host was
loaded (load average 7-18 on 16 cores with five concurrent rustc), which
inflates the absolute figures.

Three named components of the aggregate:

1. Two projection full passes, 11738 ms and 6139 ms, both carrying an
   `org.initial_scan.ingest` span. The second is the `oversized [LEAK]` pass the
   navigation entry blamed for navigation; it is really a boot pass ingesting
   `Now.org` as a single 2649-operation batch.
2. 128 `matview_ddl` events at boot. These are the same per-block materialized
   views that make navigation slow, paid up front. See the navigation entry for
   the view shape and the `backend_engine.rs:674` anchor.
3. A concentrated per-file tail, unchanged from the original report:
   `boot_file` p50 15 ms with max 15707 ms, `boot_parse` p50 12 ms with max
   11253 ms over 129 files.

Open question worth resolving before anyone sizes this work: the log reports
`files=129`, but the vault holds 1029 `.org` files. Either the initial scan is
scoped far more narrowly than the vault root or 900 files are silently skipped.
That changes what 39.5 s means.

## Missing piece

No test boots on a vault of this size. The keystone's vaults are small enough
that a per-file tail cannot form, and `boot_ingest_total` has no budget rung of
its own — the 200 ms diagnostic threshold it is compared against is the
interaction SLO, which is the wrong budget for a cold scan and produces a
warning that is true but unactionable.

## Remedy

Open. Two separable pieces:

1. Give boot its own budget, scaled to vault size, so the diagnostic says
   something a reader can act on instead of comparing a cold scan to an
   interaction SLO.
2. Name the tail. A 16.9s single file and a 9.3s single view are two concrete
   defects hiding inside one aggregate, and neither needs an architecture
   change to find.
