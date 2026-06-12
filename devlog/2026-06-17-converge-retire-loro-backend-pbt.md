# F2 convergence: un-ignore the harness + first blessed-slice retirement (loro_backend_pbt)

Date: 2026-06-17
Worktree: `.claude/worktrees/backlog-doc-update` (jj)

## Goal

Continue porting PBT functionality to the new convergence architecture along a
**deletion-first** path: prefer moves where old code can be eliminated *soon*,
accept fewer tests meanwhile, never lose complex code that should survive.

## Key structural finding

`declare_pbt_convergence!` is `__declare_pbt_full_slice!` with a **generated,
shrinkable** `any_valid_wiring()` `init_state` instead of a pinned
`Just(fresh_reference_state($wiring))`. A blessed per-Wiring slice
(`component_pbt!` / `declare_pbt_slice!`) is therefore just the convergence harness
**pinned to one point** in the generated wiring powerset. So the convergence
harness *structurally subsumes* every per-Wiring blessed slice — the complex code
(transition alphabet `aggregate_transitions`, `run_invariant_registry`, `E2ESut`)
is shared, not duplicated in the slice files. Retiring a generic slice loses no
complex code; the only asset is its `.proptest-regressions` corpus.

## Changes

1. **Un-ignored the convergence harness.** Removed the `test_attrs: [ #[ignore …] ]`
   from `declare_pbt_convergence!` (`pbt/slice.rs`). It now runs under a default
   `cargo test`. Per user decision, `wiring_axes()` defaults are **left unchanged**
   (`{Loro, Org, Turso}`, Turso down-weighted 15%) — speed is handled separately and
   `HOLON_PBT_WIRING_AXES` is the fast-scoping knob (e.g. `"Loro;;"`).

2. **Retired `loro_backend_pbt`** (the first blessed slice, pinned `{Loro}`):
   deleted `tests/loro_backend_pbt.rs` + `tests/loro_backend_pbt.proptest-regressions`.
   Its `{Loro}` wiring is now a generated draw of the convergence harness, which runs
   the same Loro invariants (`inv-loro-no-errors`, `inv-loro-children-match-ref`,
   `inv-blocks-match-ref/loro`) whenever Loro is wired.

3. Doc-comment fixups in `memory_slice.rs`, `slice.rs`, `justfile` (the `pbt-slice`
   example) where they named the now-retired slice.

## Validation (all green)

- `HOLON_PBT_WIRING_AXES="Loro;;" PROPTEST_CASES=2 … --test subsystem_convergence_pbt`
  → **ok, 0 ignored**, 20.9s (was `#[ignore]`d before).
- Default-scope (no env var) `PROPTEST_CASES=2 … --test subsystem_convergence_pbt`
  → **ok, 0 ignored**, 10.8s.
- `--test loro_backend_pbt` (pre-deletion, all 8 cases incl. its full regression
  corpus) → **ok**, 87.8s ⇒ its seeds carry **no live bug**, safe to retire.

## The repeatable retirement recipe (for the next slices)

1. Run the slice green (its `.proptest-regressions` must all pass = no live bug).
2. Confirm the convergence harness covers the slice's wiring + invariants
   (scope with `HOLON_PBT_WIRING_AXES` to that wiring; check the invariants run).
3. Delete `tests/<slice>.rs` + its `.proptest-regressions`; fix doc/justfile refs.
4. Run convergence green at that scope.

**Seed-corpus note (no silent caps):** a retired slice's regression seeds do *not*
replay verbatim under convergence (its `init_state` now includes the generated
wiring, so the seed encoding differs). The bugs they guarded are fixed (the slice
is green at deletion), so the corpus is retired with the slice rather than ported.

## Next candidates (Turso-wired, heavier — same recipe)

`storage_consistency_pbt`, `general_e2e_pbt` (sql_only), `cdc_delivery_pbt`,
`split_block_content_pbt`, `extended_gen_pbt`, `layout_override_pbt`,
`org_render_fixed_point_pbt`. Scenario/regression slices (loro_restart_unseeded_vault,
loro_content_drop, bidirectional_sync, peer_conflict, cross_frontend, …) carry
bespoke setup/generators = complex code → **keep / port deliberately**, not subsume.
