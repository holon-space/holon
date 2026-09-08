---
id: 2026-09-08-the-boot-scan-projects-every-file-with-a-full-document-walk
date: 2026-09-08
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The Loro subscription that feeds the incremental projection was registered by
  the boot-gated sync controller, so the org initial scan's per-file flushes
  always found an empty fact queue and every one walked the whole accumulated
  tree — 121 full walks for a 120-file boot.
---

## Bug

Found by measurement (lane `nav-latency`) while chasing the two OPEN latency
entries, not by any test. On a synthesized 120-file / 2793-block vault —
the shape of Martin's vault, 121 files and 2621 headlines —
`scripts/measure_latency.py` over the boot log reported:

    PROJECTION MODE ATTRIBUTION
      incremental (O(changed) fast path): 20
      full (reseed walk):                 121
      full-pass reasons:  coldboot  121  [seed]

    PIPELINE STAGE COST
    projection (full pass)      141   510.0   997.0  1991.0  ms
    projection (snapshot only)  141   258.0   562.0  1188.0  ms
    boot_ingest_total             1        110233 ms

Every one of the scan's 120 per-file flushes took the full-document walk. Each
walk reads the WHOLE accumulated tree, so file *k* costs O(k·22) and the boot
costs O(N²) in vault size. The 20 incremental passes are the post-boot edits,
which took the fast path correctly.

## Root cause

`LoroProjection::project` entered the O(changed) fast path only when
`seeded && armed`, and the fast path's input — the pending-facts queue filled by
`doc.subscribe_root` — was registered inside `LoroSyncController::start_gated`.

Both halves defeat the scan:

- `arm()` runs only AFTER the initial scan (crates/holon-orgmode/src/di.rs), so
  `armed` is false for every file. The `armed` flag's own documentation says it
  gates DELETES and that "creates/updates are never gated" — the fast-path
  conjunction contradicted that.
- The controller that registered the subscription is held behind the boot
  `SyncGate` until the scan finishes. `start_gated`'s comment claimed "the
  subscription is still registered up front, so the scan's flushes drain real
  pending facts instead of falling back to full reseeds". A probe counting
  callback invocations measured `fired=0` for all 120 boot passes: the claim was
  false, and nothing tested it.

So the scan drove the projection directly through `DownstreamProjection::flush`
while its incremental input leg did not yet exist.

## Missing piece

Nothing counted projection passes by mode. The `holon_latency` projection event
carries `mode` and `reason`, but it is opt-in and only a log parse could see it,
so a boot that took 121 full walks looked exactly like a boot that took one.
`projection_stats` tallied passes, ops and milliseconds — every quantity that
moves with machine load — and not the one ratio that does not.

## Remedy

Fixed. `LoroProjection` now owns the wake signal and its two `subscribe_root`
registrations (`install_doc_subscriptions`, idempotent), installed by the DI
provider that builds the projection — before anything can flush it. The fast
path's gate drops `armed`; an unarmed batch carrying a DELETE still routes to
the full walk, so the delete gate keeps its single implementation.

`projection_stats::Stats` gains `full_passes`, and
`crates/holon-integration-tests/tests/vault_scale_interaction_latency.rs`
asserts a cold boot takes at most two full walks. That assertion is the covering
test and it is load-insensitive, unlike every wall-clock rung beside it.

Measured on the same 120-file corpus, before → after:

| | before | after |
|---|---|---|
| full reseed walks | 121 | 1 |
| projection snapshot p50 | 258 ms | 8 ms |
| `boot_ingest_total` (one run each) | 110233 ms | 58768 ms |

Wall-clock boot on that corpus is NOT a clean before/after number — this
machine's spread swamps the difference. `scripts/boot_distribution.py <log-dir>`
classifies every recorded run by regime (one full walk = fixed, a walk per file
= the defect) and reports, for 120 files / 2793 blocks:

    one-full-walk (fixed): n=6 min=41.9s median=59.5s max=151.4s
      samples: 41.9, 43.3, 58.4, 60.6, 90.8, 151.4
    full-walk-per-file (defect): n=2 min=74.5s median=93.5s max=112.5s
      samples: 74.5, 112.5

The fixed distribution's max exceeds the defect's max, so a single wall-clock
sample proves nothing here; an independent verifier run measured 79.2 s, inside
that range. The *ratio* — full walks per boot — is what moved, and it is what
the covering test asserts. A quiet-machine three-rung run is still owed.

Red log, with the DI registration reverted to the old ordering (40 files):

    [vault-scale] projection passes: 41 total, 41 full (952 ops, 2446ms in snapshots)
    panicked at vault_scale_interaction_latency.rs:318:
    boot took 41 full-document reseed walks for 40 files. A cold boot needs ONE;
    a walk per file is quadratic in vault size, because each walks the whole
    ACCUMULATED tree.

This does NOT close the two latency entries it was found under — see
[[2026-09-03-navigate-latency-is-twenty-seconds-at-real-vault-scale]] and
[[2026-09-02-projection-misses-the-slo-on-the-real-vault]], which stay OPEN
because the interaction SLO is still missed by a cost this fix does not touch.
