# Verify: keystone-reboot Inc 1 (commit 820f26f9) — CONFIRMED with gaps

Scratch tree: /private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/verify-reboot
(git archive of 820f26f9b27b02978957f79c5394b9c70f7d31e0 from the PRIMARY repo; non-empty, capabilities.rs carries `reboot`)

## C1 Behaviour-neutral refactor — CONFIRMED
- `jj diff -r 820f26f9 --stat` touches exactly the 5 named files (631 ins / 309 del).
- Every `pub fn compose_sut*` signature UNCHANGED; the new param `existing_frontend` is on the
  private `compose_sut_seeded_impl` only, plus a NEW `pub async fn compose_sut_over_existing`.
- Two fail-loud asserts guard the new arm (no seeds, ViewModel projection required).
- `HeadlessStore` / `BootedSession` / `BootParams` split as claimed; `boot_session` re-callable.
- `install_observability_caps` extracted verbatim and called from BOTH `boot_and_seed_wide`
  and `reboot_wide`.
- `ComposedSut::rebooted` replaces ONLY `caps` + `handle` and bumps `tick`; `resolver`,
  `burned`, `redo_burned`, `foreign_ids`, `telemetry`, `engaged`, `rt` carry by construction
  (`fn rebooted(mut self, …) -> Self`).
- No invariant or transition body changed anywhere in the diff (all other hunks are
  mechanical `self.field` -> `self.field()` accessor rewrites).

## C2 Anti-double-seed teeth — CONFIRMED (with a timing caveat)
- `reboot()` (components.rs:992) snapshots `store_block_ids()` before shutdown and again after
  the new boot, `assert_eq!` with a gained/lost diff message.
- `store_block_ids` (components.rs:560) -> `all_blocks()` -> LIVE
  `self.engine().db_handle().query(BLOCK_RAW_SNAPSHOT_SQL)`. NOT a cache; `engine()` resolves
  through `booted()`, so `after` reads the NEW boot's engine. Teeth are real.
- Reuse branch skips seeding: `compose_sut_over_existing` passes `&[] , &[]` and the impl
  asserts both seeds empty.
- `scaffold_ids` is NOT re-snapshotted: `rebooted` never touches it and `reboot_wide` returns
  only `(CapMap, WideHandle)`. Confirmed from code.
- CAVEAT (gap, not refutation): the second boot's settle is capped at `BootParams.settle`
  (300 ms for the composed builder) and is explicitly tolerant (`let _boot_converged`). A
  re-seed that lands AFTER that window would leave `after == before` and the assert would pass
  vacuously. The teeth are sound against a synchronous re-seed only.

## C3 Untested rebuild seam — GAP CONFIRMED
- NO unit test added for `reboot` / `boot_session` (grep over the whole tree: the only
  `*reboot*` tests are pre-existing `TestEnvironment`-based ones in boot_suite/store_suite).
- `fn is_reboot` has exactly ONE definition — the `false` default in harness.rs:254. No slice
  overrides it and no `Reboot` transition variant exists, so `rebooted()` / `reboot_wide()` /
  `compose_sut_over_existing` / `HeadlessFrontendComponent::reboot` are entirely DEAD in Inc 1.
  Nothing exercises the rebuild path until Inc 2.

## C4 Gates — CONFIRMED (all three reproduced by me on the 820f26f9 tree)
- `cargo check -p holon-integration-tests --features holon-integration-tests/pbt --all-targets`
  -> exit 0, `Finished dev profile in 60m 02s`, 0 errors. (scratchpad/check.log)
- `just keystone-smoke` -> `test result: ok. 4 passed; 0 failed`. (scratchpad/gates2.log)
- `just hand-authored` -> `test result: ok. 9 passed; 0 failed` in 1758.70 s. (same log)
- Lane's `--workspace` log `lane-logs/inc1-ws-12766.log`: `Finished dev profile in 96m 09s`,
  0 lines matching `^error`. TIMESTAMP CAVEAT: it finished 15:29 having started ~13:53, i.e.
  it BEGAN before the commit's committer timestamp 15:05. Weak evidence for the final tree;
  my own crate-level check covers the changed crate, the rest of the workspace is unchanged
  by the diff so the risk is low.
- NOTE (my harness error, not the lane's): a first attempt passing the gates as
  `bash -c '…'` through with-build-slot.sh silently ran `just` with NO arguments — it printed
  the recipe list and exited 0 (a false green, SMOKE_EXIT=0 with zero tests). Re-run via a
  script file produced the real results above. A second attempt also died on an sccache
  daemon crash (`could not compile notify-types`), cured with `RUSTC_WRAPPER=`.

## C5 Fail-loud / correctness details — CONFIRMED
- `SutAppLifecycle::reboot` in capabilities.rs has NO default body (`async fn reboot(&self);`),
  so a no-op default is impossible. Only ONE impl exists (`HeadlessFrontendComponent`), so
  nothing else silently no-ops.
- `ComposedSlice::reboot`'s default returns `None`, and `rebooted()` `.expect(...)`s it with a
  message telling the slice to narrow the transition out. Fail-loud, not no-op.
- `HeadlessStore::org_root` and `HeadlessFrontendComponent::org_root` both read the FIELD
  (`&self.org_root`, `&self.store.org_root`) — no recursion.
- `driver()` vs `driver_concrete()`: the 4 `self.driver().as_ref()` sites are all
  argument-position temporaries in a single statement, so the `Arc` outlives the `&dyn` borrow
  (compiles; verified by the green check). `driver_concrete()` is used exactly where an
  inherent `ReactiveEngineDriver` method is needed.

## Defects / risks found (evidence only — no fixes made)
1. DEAD CODE introduced by this commit, reproduced in my check.log:
   `warning: methods 'org_root' and 'db_path' are never used` (HeadlessStore, components.rs:181/185)
   `warning: method 'store' is never used` (components.rs:1045)
   `db_path()` being unused means `boot_session` duplicates the path inline as
   `temp_path.join("test.db")` — two sources of truth for the DB path.
2. THE OLD BOOT IS NOT DROPPED WHEN `boot_session` RUNS. In `reboot()`:
   `let old = self.booted(); … drop(old);` only drops a CLONE — `self.boot` still holds the
   original `Arc<BootedSession>`, and the pre-reboot `CapMap` holds further clones of
   engine/reactive/driver (the harness replaces `sut.caps` only AFTER `S::reboot` returns).
   So the OLD ReactiveEngine / FrontendSession / file-sync + clock background tasks are alive
   and running against a SHUT-DOWN db_handle for the whole duration of the second boot, over
   the SAME InMemoryFileSystem and org root. Latent second-writer / error-spew hazard;
   unobservable in Inc 1, must be probed in Inc 2.
3. The reboot branch in `harness::apply` returns BEFORE the timed window, so a reboot tick
   records no settle latency, runs no `sut_ids` before/after bookkeeping, no wedge timeout,
   no `invalidate_render_caches`, and no telemetry. `inv-settle-budget` on a reboot tick will
   read whatever the freshly-installed `ComposedSettleLatency` holds (empty). Inc 2 risk.
4. `install_observability_caps` calls `ReseedObserver::global().reset()` and
   `reset_missing_declared_warnings()` on EVERY reboot, discarding the pre-reboot half of the
   case's reseed / declared-column attribution. The lane documents this as intentional; it
   does weaken those two invariants for any case containing a Reboot.

## Verdict
CONFIRMED — every claim I checked was reproduced from evidence I produced in this session.
The refactor is behaviour-neutral and the gates are genuinely green on the commit's tree.
The teeth exist but are only as strong as a 300 ms bounded settle, and the whole rebuild seam
is dead + untested until Inc 2 (defects 1-4 above are the things to probe there).
