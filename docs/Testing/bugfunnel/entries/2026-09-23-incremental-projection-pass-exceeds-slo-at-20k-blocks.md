---
id: 2026-09-23-incremental-projection-pass-exceeds-slo-at-20k-blocks
date: 2026-09-23
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  At 20,000 blocks, a Loro-to-SQL projection pass that applies one op takes
  about 0.3-0.4 s in the RELEASE profile. That is above the 200 ms
  interaction-to-visible SLO. Every latency gate runs on a small vault, so no
  gate sees it.
---

## Bug

Found by an agent measurement (the coordinator of the savefix chain d0de9ed2..75c6a712
asked for it), while the agent found the cost of the `save_all()` that d0de9ed2 added
to each projection pass. The per-pass save costs about 45 ms. The rest of the pass
is slow without it:

| 20,000-block synthetic vault, release profile | with the per-pass save | without it |
|---|---|---|
| `save_all` in each incremental pass, edits 2-12 | median 44.9 ms (42.3-62.3) | - |
| total 1-op incremental pass, edits 2-12 | median 365 ms (324-413) | median 363 ms (280-398) |
| first edit after ingest | 496 ms (save 123 ms) | 479 ms |
| ingest full pass, 20,034 ops | 13.5 s (save 236 ms) | 12.6 s |

- **Build:** `cargo build --release -p holon-integration-tests --example crdt_incr_bench`
  (profile `release`: thin LTO, codegen-units 4), on a scratch copy of 75c6a712.
- **Measurement:** two env-gated log lines in a scratch copy of
  `crates/holon-loro/src/loro_sync_controller.rs`:
  - one line times `save_all` in `emit_ops`;
  - one line logs `t0.elapsed()` of the pass beside the `[LoroProjection] applied` log.
  - The lines are not in the repo.
- **Run:** `HOLON_SOAK_SEED_BLOCKS=20000 HOLON_BENCH_EDITS=12`. Each run is one
  single-field `set_field` edit on a middle block, then Loro quiescence, 12 times.
  The vault is a temp dir made by `TestEnvironmentBuilder`, not Martin's vault.
- **Host load:** 1-minute load average 23.0 (with) and 18.8 (without) at the
  start of each run. That is a busy host, so read absolute numbers as an upper
  range, not a floor.
- **Result:** the two columns do not differ beyond noise. So the slow pass is not
  caused by d0de9ed2.

## Root cause

Not yet attributed. The pass that applies one op is called `incremental` (O(changed)). It
is `LoroProjection::project` → `emit_ops`
(`crates/holon-loro/src/loro_sync_controller.rs:923` and around `:1440` at 75c6a712).
At 20k blocks it still costs about 0.3 s outside the save. So the time goes to work
that scales with vault size, somewhere in the pass or in the sink apply it waits
for (`self.consolidator.apply(...)`, projection stats around `:1525`).

- **Next step:** split the pass into its stages: incremental change extraction,
  sink apply (Turso write + IVM matview maintenance), and the read-model publish.
  Measure each at 5k, 10k and 20k blocks to find the stage that grows with N.
- **Candidates:**
  - matview maintenance that is not O(delta);
  - a full scan in the sink;
  - a watermark/frontier computation over the whole oplog.

## Missing piece

No latency gate runs at vault scale:
- `just latency-slo-gate` (`latency_slo_gate.rs`) boots the keystone's wide wiring
  on a nearly empty vault (the setup prefix creates about 150 blocks).
- `just latency-gate` replays `latency-ratchet.jsonl` on the keystone boot vault.
- `just soak` does drive 5-10k blocks with CRDT on, but it is a manual measurement.
  It is not a landing-gate step, and it has no failing threshold.

So a vault-scale pass above the SLO can land with every gate green. A soak rung
at 20k blocks with a p95 threshold on `stage=e2e` would catch it (gate or ratchet,
calibrated per `docs/Testing/latency-ceilings.txt`). Run it in the release profile,
because the test profile is about 11-12x slower and would swamp the threshold.

## Remedy

OPEN.
1. Attribute the per-stage cost as described above.
2. Fix the stage that grows with N.
3. Add a release-profile vault-scale latency rung so this cannot escape again.

The savefix chain does not need to wait for this. Its per-pass save adds about
45 ms to a cost that is already above the SLO.
