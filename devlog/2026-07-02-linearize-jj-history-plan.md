# Plan: Linearize jj history across bookmarks & workspaces

**Status:** APPROVED (plan-only turn). Decisions: foundation-first order · fold sryr & delete stale bookmarks · flatten spec-0008 merge · execute in a later turn.

## Snapshot (2026-07-02)

No existing conflicts or divergent commits — all conflicts below are *introduced* by stacking. Everything fans out from `main vpvv 77387a04` → `ypyt 107de035` → `vmql e0681c3e`.

```
vmql ─ nvpw…onlz              model-invariants-0008 (8)  ─ merged via nvsw
     └ vxmu 81318476
        ├ sryr 52734571 (parked "entity_uri cleanup" wip)
        │  ├ nylu…qlpy(→tllm empty)   blockeventstorm-hotspots@ (12)
        │  └ yxzy…qnru                 pbt-target-arch@ (17)
        └ nvsw 4324ce8b = MERGE(vxmu,onlz)  spec-0008 Phases 0-3
           └ llnt ─ mmuv ─ mvzn cc98e5a7
              ├ zxpo 788387f8            default@ (EpochFlipRejected)
              │  └ rzyl…zxpy             senior-review-loro-iroh@ (6)
              └ sxrt…wwzt(→kplx/nxvl empty)  crate-decoupling@ (9)
```

Segment tips: crate-decoupling = `wwzt e3e83eee`; blockeventstorm = `qlpy 34960fc2`; pbt = `qnru b0fe089c`; senior-review = `zxpy`. Empty WC tips to abandon: `tllm`, crate-decoupling WC (`nxvl`).

## Target linear order (bottom → top)

1. `main vpvv → ypyt → vmql` (already linear)
2. spec-0008 trunk, merge flattened: `vxmu → nvpw…onlz → llnt → mmuv → mvzn → zxpo`
3. crate-decoupling `sxrt…wwzt` (9)
4. `sryr` (folded) → blockeventstorm `nylu…qlpy` (12)
5. pbt-target-arch `yxzy…qnru` (17)
6. senior-review-loro-iroh `rzyl…zxpy` (6)

## Conflict forecast (measured file overlaps)

- **Universal hotspot:** `crates/holon-integration-tests/src/pbt/frontend_slice/components.rs` — touched by trunk, decoupling, blockeventstorm, pbt. Expect a conflict at nearly every rebase; resolve preserving all four intents.
- `Cargo.lock` / `frontends/holon-worker/Cargo.lock` — collide everywhere; regenerate with `cargo build`, don't hand-merge.
- trunk∩decoupling (14): holon-core `lib.rs`/`traits.rs`, holon-orgmode `di.rs`, `backend_engine.rs`, holon-app `wiring.rs`/`loro_seams.rs`/`Cargo.toml`, holon-turso `lib.rs`, `sql_block_operations.rs`, `event_infra_module.rs`.
- trunk∩blockeventstorm (11): `traits.rs`, `sut_handle.rs`, `test_environment.rs`, `di.rs`, `backend_engine.rs`, `widget_state.rs`, `Sync.md`, `Architecture.md`.
- trunk∩pbt (4): `components.rs`, `widget_state.rs`, `random_pbt_sim.rs`, `sim_windowed_replay.rs`.
- pbt∩blockeventstorm (3): `components.rs`, `widget_state.rs`, `*.proptest-regressions`.
- senior-review (15 files, sharing subsystem) — low; main risk is write-routing vs crate-decoupling.

## Execution steps (jj)

Run from the **default workspace**. Record rollback point first: `jj op log --limit 3` (any step reverts via `jj op restore <id>`).

**Pre-flight — multi-workspace hazard** (per memory `model_invariants_0008_merged`): the 4 extra workspaces (pbt-target-arch, crate-decoupling, blockeventstorm, senior-review-loro-iroh) will have their commits rewritten. Ensure no running app/test holds `index.lock` in any of them before starting.

### Step 1 — Flatten the spec-0008 merge
```
jj rebase -s nvpw -d vxmu          # model-inv-0008 stack onto vxmu → onlz now descends vxmu
jj diff -r nvsw                    # inspect the (now trivial) merge
# if empty: jj abandon nvsw  (llnt reparents to onlz automatically)
# if it carries resolution content: jj rebase -r nvsw -d onlz (becomes single-parent)
```
Risk: LOW — `vxmu` is docs/warning cleanup; the onlz stack is the real work the merge already reconciled.

### Step 2 — crate-decoupling onto default tip
```
jj rebase -s sxrt -d zxpo
```
Resolve the 14-file overlap (see forecast). `jj abandon` the empty decoupling WC tip. Segment tip = `wwzt`.

### Step 3 — blockeventstorm (+ folded sryr) onto crate-decoupling
First make the sryr subtree linear (pbt on top of blockeventstorm) *before* moving it, so each commit resolves against the new base exactly once:
```
jj rebase -s yxzy -d qlpy          # pbt onto blockeventstorm tip, still on old sryr base (cheap: pbt∩bes = 3 files)
jj rebase -s sryr -d wwzt          # whole linear sryr→bes→pbt chain onto crate-decoupling
```
Resolve conflicts in commit order (sryr cleanup first — small; then blockeventstorm 11-file overlap; then pbt 4-file overlap). `jj abandon` empty `tllm`.

### Step 4 — senior-review-loro-iroh onto pbt tip
```
jj rebase -s rzyl -d qnru
```
Resolve low-risk sharing conflicts (write-routing).

### Step 5 — cleanup
```
jj bookmark delete model-invariants-0008 block-event-storm decouple-phase1
jj workspace forget pbt-target-arch crate-decoupling blockeventstorm-hotspots senior-review-loro-iroh   # optional: collapse to one workspace
```
Leave `default`/`senior-review-loro-iroh-sharing` bookmarks pointing at their new locations, or delete once satisfied.

### Verification (after EACH segment, not just at the end)
Shell is nu — build via bash to avoid the `out+err>` false-green trap (memory `partial_e5`):
```
bash -c 'cargo build --workspace 2>&1 | tee /tmp/lin-build.log'
bash -c 'cargo nextest run --workspace 2>&1 | tee /tmp/lin-test.log'   # heavier segments
```
Regenerate Cargo.lock by building rather than resolving the conflict by hand.

### Conflict-resolution method
Use the `jj-resolve` skill: mergiraf first (`jj resolve`), manual for `components.rs`/`widget_state.rs`. Never `.ok()`/swallow — fail loud (CLAUDE.md). If a resolution is non-obvious, stop and surface it.

## Deferred decision (needs explicit OK — outward-facing)
`main` is 1 ahead / 46 behind `origin`. Whether to fast-forward `main` to the new linear tip and `jj git push` is **not** part of this plan — confirm separately before any push.
