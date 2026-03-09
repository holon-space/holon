# Lever 3 landed: the 3-projection convergence settle — and the peer-order bug it disclosed

**Worktree / jj workspace:** `composed-pbt-boot-parallelism`
(`/Users/martin/Workspaces/pkm/holon/.claude/worktrees/composed-pbt-boot-parallelism`).
**Base:** `sequential 1..8` + flat settle (levers 1 & 2 previously reverted).
Predecessor handoff: `scratchpad/handoff-lever3-convergence-settle.md`;
memory `composed_pbt_boot_lever3_diagnosis_2026-07-03.md`.

## What landed (the deliverable — DONE, green at 1..8)

Replaced the flat per-transition `sleep(SETTLE)` in the composed keystone PBT with an
**async 3-projection convergence settle** covering **all three** projections the invariants
read (Turso CDC + Loro + org), capped at `SETTLE` (150 ms) so it never over-waits vs the old
sleep but returns fast when quiescent.

Files:
- `pbt/composed/harness.rs` — new `ComposedSlice::settle_after_apply(&Handle, &CapMap)` trait
  hook (default = `sleep(SETTLE)`); the `apply` loop calls it instead of the raw sleep
  (split the `&sut.handle` / `&mut sut.caps` borrow before `block_on`).
- `pbt/composed/wide_e2e.rs` — `WideHandle { engine, frontend }` (was `type Handle = ()`);
  `converge_projections()` = CDC watermark stable for `pbt_quiet_floor()` → Loro
  `last_synced_frontiers()==oplog_frontiers()` → org `OrgSyncIdleSignal::wait_quiescent`, all
  bounded by one shared deadline. `boot_and_seed_wide` now returns the handle and converges the
  post-`NavigateFocus` boot settle too. `WideE2E::settle_after_apply` override + `WideHandle::from_bundle`.
- `pbt/frontend_slice/components.rs` — added `org_idle_signal()` accessor (lazy DI resolve,
  mirrors `loro_sync_handle()`).
- Threaded the handle through every `boot_and_seed_wide` caller (structural_pbt.rs ×13,
  task_state_storage_coherence.rs) and the windowed harness
  (`windowed_composed_sut` gained a `WideHandle` param; gpui `windowed_wide.rs` + tui `pbt_main.rs`
  build it via `WideHandle::from_bundle(&bundle)`).

**Validation:** green at `1..8` — `PROPTEST_CASES=1` (baseline) and `=4` (with peer/external
seeds) both PASS (~102 s). Lib+tests, gpui tests, tui tests all `cargo check` clean.

**Perf:** chrome-trace on the main proptest thread — the old flat ~152 ms `query→query`
per-transition settle gap is **gone**; only the ~303 ms boot settle (`new_with_loro` 300 ms,
`components.rs:~223`, untouched — one per case) remains as a `query→query` gap. Per-transition
waits now converge instead of flat-sleeping.

## What did NOT land: lever 1 (`sequential 1..40`) — blocked by a SEPARATE bug

Re-running `1..40` at `CASES=16` **FAILS** — but **not** on the settle. It fails on
`inv-loro-children-match-ref`, a **deterministic peer-sibling-order divergence** (reverted the
test back to `1..8`; did NOT persist the red seed into the regressions file).

**Minimal case** (shrunk): under `block:parent`,
1. peer 0 `ApplyMutation::Create block:peer-…-0000-0003` (created FIRST),
2. peer 1 `ApplyMutation::Create block:peer-…-0001-0005` (created SECOND),
then both `SyncWithPeer`.

- **Loro** (`inv-loro-children-match-ref`, reads the tree's fractional index) →
  `[…0003, …0005]` = **insertion order** (peer 0 first).
- **Oracle** (`assign_reference_sequences_canonical`, `org_utils.rs:300-305`) ties equal-sequence
  siblings by **id string**; `block:peer-6346328f…` < `block:peer-dc075d84…`, so →
  `[…0005, …0003]` (peer 1 first).

Both blocks are present (set-equal, only order differs) and Loro CRDT order is
timing-independent → **this is not a settle race**. The convergence settle waits for full Loro
quiescence, so it reads Loro's *final* order; more settle cannot change it.

**Direction — RESOLVED with a deterministic diagnostic** (boot `full_headless` so both stores are
present; drive two concurrent peer `Create`s under `block:parent` with ids where id-order reverses
create order; dump Loro vs SQL order). Why the `1..40` failure only showed `inv-loro`: that draw is
a **Loro-only wiring** (`any_valid_wiring()` shrinks toward Loro-only), so `SutSqlProjection` is
absent and `inv-live-children-match-ref` **deselects** — Turso isn't exercised at all. Forced to
full_headless the diagnostic shows:

| source | order under `block:parent` | why |
|---|---|---|
| Loro `loro_children_of` | `[zzz, aaa]` — **insertion/fractional-index** | true CRDT order |
| SQL `sorted_children` (`ORDER BY sort_key, id`) | `[aaa, zzz]` — **id order** | `sort_key` **tied** → id fallback |
| oracle (`assign_reference_sequences_canonical`) | `[aaa, zzz]` — **id order** | id-tiebreak |

`block_raw` rows for both peer blocks are **identical in every ordering field** (tied `sort_key`).
So Turso doesn't "agree with Ref" on the merits — it **coincides** with the oracle only because
**both share a projection-totality gap**: the Loro fractional index never propagates to distinct
SQL `sort_key` for peer-merged blocks, so SQL falls back to id-order, and the oracle models that
degraded state. **Loro alone holds the true order.** This is exactly the bug class
`inv-live-children-match-ref`'s doc names ("fi never reaches SQL sort_key, left at default").
Memory precedent [`composed_bulk_split_sibling_order_bug`]: sort_key/fi rank is canonical,
id-order collapse is the bug.

**Recommended fix (insertion order is canonical):** propagate the Loro fractional index into SQL
`sort_key` for the peer-merge projection path (prod fix) so SQL shows `[zzz, aaa]`, then update the
oracle to model insertion order for peer-created siblings. The lower-effort alternative — collapse
Loro's read to id-order — is **wrong**: it would destroy the authoritative CRDT order.

## Next steps

1. Fix the peer-sibling-order bug (Loro adapter re-canonicalization most likely), then re-land
   lever 1 (`1..8` → `1..40` at `general_e2e_composed_pbt.rs:31`) and validate GREEN at `CASES≥16`.
2. Optional boot-settle win (untouched, ~303 ms×cases): converge the `new_with_loro` 300 ms
   sleep (`holon-frontend/.../components.rs:~223`) + the sync-handle poll (`builder.rs:375`).

## Repro

`bash -c 'PROPTEST_CASES=16 cargo nextest run -p holon-integration-tests --test general_e2e_composed_pbt --no-capture | tee /tmp/run.log'`
(set the test to `1..40` to reproduce the peer-order red). Always `bash -c '… | tee'` — nu's
`out+err>` redirect gives false-greens.
