# Handoff — Lever 3: parallelize composed-PBT SUT boot (the 90% cost)

**Context:** performance work on the ONE composed keystone PBT
(`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`,
`ComposedSut<WideE2E>`).

**Status of the two "easy" levers — BOTH ATTEMPTED, BOTH REVERTED.** They are not
as free as they look, and the reason IS this task:

- **Lever 1 (amortize boot: `sequential 1..8` → `1..40`)** — reverted. It is NOT
  "zero risk". Deeper sequences accumulate more blocks (~51 in one failing case),
  and the **fixed 150 ms settle does not scale with block count** — the org/Loro
  projection lags the oracle and `inv-blocks-match-ref/org` diverges (a block with
  empty content under `block:c1`). The old `1..8` simply kept block counts low
  enough that 150 ms sufficed. **Lever 1 is gated on a settle that scales.**
- **Lever 2 (convergence-settle instead of the flat 150 ms sleep)** — reverted.
  Polling only the Turso CDC watermark returns before Loro peer-sync finishes,
  diverging concurrent-peer CRDT seeds (details in "Bonus scope" below).

Both failures are the same root problem: **fixed/partial settle windows don't
scale.** So the real unlock — and this handoff — is the boot + convergence model.
The only things kept on the branch are the **diagnosis**, the **`pbt-perf-diagnosis`
skill**, this handoff, and a **fix to `scripts/analyze-chrome-trace.py`** (recover
truncated traces). The test itself is untouched (still green at `1..8`).

## The one-paragraph diagnosis (measured, not guessed)

The test is **wait-bound, not CPU-bound**: `/usr/bin/time -p` on a run shows
**~85 % idle wait, ~15 % CPU** (cases=1: real 111.8 s, user 14.0 s, sys 3.1 s).
**Boot dominates: ~90 % of wall is per-case SUT construction** — each boot is
**~8.6 s wall but only ~1 s CPU**, i.e. **~7.5 s of every boot is the process
asleep**, waiting on serialized async init. Transitions are cheap (~0.2 s, of
which the fixed 150 ms `SETTLE` sleep is most). So the entire game is boot. Full
breakdown + how to reproduce the numbers: run the `pbt-perf-diagnosis` skill or
see the method at the bottom.

## Where the boot time goes (from a chrome-trace, cases=1, 12 boots)

Build with `--features chrome-trace`, then
`python3 scripts/analyze-chrome-trace.py <trace.json>`. The `di.*` spans all show
`n=12` (= 12 boots), so divide by 12 for per-boot cost:

| span | total (12 boots) | per boot | what it is |
|---|---|---|---|
| `di.create_backend_engine` | 7.45 s | ~620 ms | Turso `BackendEngine` construction |
| `execute_ddl` | 4.87 s | ~405 ms | Turso **matview DDL** (the IVM graph) |
| `di.schema_module` | 4.49 s | ~375 ms | schema module setup (n=120 = 10/boot) |
| `di.preload_startup_views` | 0.92 s | ~77 ms | preload matviews |
| `di.create_initialized_engine` | 1.89 s | ~158 ms | engine init wrapper |
| `di.factory.FrontendSession` | 1.21 s | ~100 ms | frontend session factory |

Plus **multi-second idle gaps** early in boot (`3.90 s`, `1.94 s`, `1.10 s`,
`895 ms`, `800 ms`) all bracketed by `org.poll_tracked_files` /
`org.poll_new_files` — the **org file-sync/watcher settling** during startup is a
big chunk of the *wait*.

**Trap:** `org.startup.arm_watcher_blocking` shows an 18.2 s span — it is NOT a
stall. It ends in `std::future::pending()` (`crates/holon-orgmode/src/di.rs:673`),
a keepalive that lives for the whole session. Don't chase it.

## The actual task

**Look at the initialization dependency graph and reshape it for maximum
parallelism by default.** The boot is a **linear `let x = …await;` chain** in
`crates/holon-integration-tests/src/pbt/composed/builder.rs`
(`compose_sut_seeded_impl`, ~line 208), flag-guarded by
`has_turso` / `has_loro` / `has_frontend`. Everything runs sequentially even
where there is no data dependency.

Concrete questions to answer:

1. **Draw the real dependency DAG of boot.** Nodes: Turso `BackendEngine`,
   the matview/DDL graph, schema modules, Loro `LoroBackend`, the
   `FrontendSession` + `ReactiveEngine`, the `LoroSyncController`, the org
   file-sync controller + watcher arming, `preload_startup_views`, the seed
   (org write + tree ingest). Which edges are *real* (B needs A's output) vs
   *incidental* (just written in sequence)?
   - Likely independent and parallelizable: **Loro backend** vs **Turso engine**
     construction; schema modules that don't chain; file-watcher arming (already
     `spawn_blocking` — is boot *waiting* on it when it shouldn't?).
   - Likely a real serial chain: **chained matviews** (Turso IVM: a matview that
     selects FROM another matview must be created after it — see the
     `turso-chained-matview-hang` skill). Confirm which DDL is actually chained
     vs independent; independent matviews can be created concurrently.

   > **The needs/provides mechanism the user remembered STILL EXISTS — and a
   > dependency-aware DDL scheduler is already built. It is just being throttled
   > to serial. This is the crux of lever 3.** Concretely:
   > - `crates/holon-turso/src/schema_modules.rs` — each schema module declares
   >   `fn provides(&self) -> Vec<Resource>` / `fn requires(&self) -> Vec<Resource>`.
   > - `crates/holon-turso/src/matview_manager.rs::ensure_view` parses each
   >   matview's `requires` from its SQL and calls
   >   `db_handle.execute_ddl_with_deps(sql, provides, requires, priority::DDL_MATVIEW)`.
   > - `crates/holon-turso/src/turso.rs::execute_ddl_with_deps` (~line 414) sends
   >   `DbCommand::ExecuteDdlWithDeps` to the DB actor, which **waits until each
   >   `requires` resource is `mark_available()` and runs ready ops by priority** —
   >   i.e. a real dependency scheduler that *could* run independent DDL as soon as
   >   deps are met.
   > - **BUT two things serialize it:** (a) `ensure_view` wraps the whole call in a
   >   global `let _ddl_guard = self.ddl_mutex.lock().await;` (matview_manager.rs
   >   ~line 364) — one DDL at a time; (b) the boot chain `.await`s each
   >   `ensure_view`/`preload` sequentially. So the graph is declared and a
   >   scheduler exists, but it never sees more than one op at once.
   > - **Lever-3 move:** feed the scheduler the *whole* matview set at once (submit
   >   all `execute_ddl_with_deps` before awaiting), and narrow/remove the
   >   `ddl_mutex` so disjoint `provides`/`requires` DDL runs concurrently. Verify
   >   the `requires`-availability gating actually preserves chained-matview
   >   ordering (it should — that's what it's for). Do NOT hand-topo-sort; reuse
   >   the existing provides/requires scheduler.

2. **Is boot blocking on the org file-sync poll loop?** The multi-second
   `org.poll_tracked_files` gaps suggest the seed/boot waits for the watcher to
   observe files via a *poll interval* rather than a direct signal. If so, make
   the initial ingest push directly (or shrink/skip the poll wait at boot) rather
   than waiting for the next poll tick.

   > **User's hypothesis to check first:** boot may be blocking because the UI /
   > first render waits for the *initial layout blocks* to arrive. If so, the fix
   > is to NOT block on them — render an empty/loading shell and **handle
   > late-arriving layout blocks** reactively (they already flow through CDC).
   > Look at the frontend initial render / `initial_widget` path and first-boot
   > seeding (`block:journals` / index.org layout — see memory
   > `first_boot_journals_seeding`). Confirm against the chrome-trace whether the
   > `org.poll_tracked_files` gaps sit on the boot critical path or on a detached
   > watcher task.

3. **Make parallelism the default, not an opt-in.** Per the repo's design rules
   (CLAUDE.md: "start with experiments to de-risk, then refactor completely, no
   parallel code paths left as a fallback"), the goal is a boot where independent
   subsystems are `tokio::join!`ed / spawned by construction, not a sequential
   chain with a fast path bolted on.

4. **Watch the numbers, fail loud.** Re-run the chrome-trace + `/usr/bin/time`
   before/after. Target: cut the ~7.5 s/boot wait substantially. The composed
   keystone PBT must stay **green** (it is a canonical correctness oracle — a
   boot race that corrupts seed state would show as invariant divergence). Do NOT
   introduce a silent fallback that hides a half-initialized SUT — surface any
   boot failure.

## Bonus scope folded in from lever 2 — a correct convergence-settle

Lever 2 (replace the per-transition `SETTLE = 150 ms` flat sleep in
`harness.rs::apply` with a convergence poll) was **attempted and reverted** —
capture the correct version here because it needs the SAME unified quiescence
signal this task builds.

- What was tried: `ComposedSlice::settle` seam + `WideE2E` override polling the
  Turso **CDC watermark** (`engine.db_handle().cdc_emitted_watermark()`), capped
  at `SETTLE`. Per-transition wall dropped 194 ms → **68 ms avg** (Nothing
  152→18, SetEdgeField 153→28) — big win.
- Why it was reverted: it **diverged the keystone PBT** on concurrent-peer CRDT
  seeds (`AddPeer, AddPeer, PeerEdit(content:"baaa"), ApplyMutation(LoroPeer 1,
  content:"faaa"), SyncWithPeer(1), SyncWithPeer(0)` → `block:parent` content came
  out as an *anagram* of the two edits). The CDC watermark goes quiet **before the
  Loro peer-sync finishes projecting**, so the snapshot caught a mid-merge state.
  The flat 150 ms sleep masked this by brute force.
- The fix: the settle must converge on **CDC watermark AND Loro quiescence**
  (`wait_for_loro_quiescence_on(sync_handle, doc_store, …)` in `test_environment.rs`),
  the same pair prod's `pre_inv16_settle` waits on. **Blocker:** the boot
  `ComposedSut`/`SutBundle` (`builder.rs:~68`) exposes `engine` but NOT the
  `frontend_sync_handle` / `LoroDocumentStore` — surface those on the bundle
  (they're locals in `compose_sut_seeded_impl`) and thread them into the settle so
  it can await both signals. Then re-validate green on the full PBT (peer seeds
  included) before keeping.

The scaffolding for this (the `settle` seam) was reverted to keep the shipped diff
= lever 1 only; rebuild it here alongside the boot-quiescence work.

## Key files & pointers

- `crates/holon-integration-tests/src/pbt/composed/builder.rs` —
  `compose_sut_seeded_impl` (the boot chain). Returns `SutBundle { caps, engine,
  … }`.
- `crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs` —
  `boot_and_seed_wide` / `boot_and_seed_wide_with_engine` (wraps the above + seeds
  the wide tree). This is what the PBT boots per case.
- `crates/holon-orgmode/src/di.rs:~660` — org watcher arming (`arm_watcher_blocking`).
- `crates/holon/src/api/backend_engine.rs` — `BackendEngine`, `db_handle()`.
- Turso matview chaining: skill `turso-chained-matview-hang`; also
  `turso-ivm-context-param-preload`.
- Boot happens once per proptest case via
  `ComposedSut::init_test` (`…/composed/harness.rs:249`) → `S::build`.

## How to reproduce the measurements

```bash
# Top-level busy vs wait (85% idle):
/usr/bin/time -p env PROPTEST_CASES=1 cargo nextest run -p holon-integration-tests \
  --test general_e2e_composed_pbt --no-capture 2>&1 | tee /tmp/run.log
grep -E '^real|^user|^sys' /tmp/run.log     # CPU = user+sys, wait = real-(user+sys)

# Wait attribution (per-thread active%, idle gaps, top spans):
CHROME_TRACE_FILE=/tmp/boot.json PROPTEST_CASES=1 \
  cargo nextest run -p holon-integration-tests --test general_e2e_composed_pbt \
  --features chrome-trace --no-capture 2>&1 | tee /tmp/chrome.log
python3 scripts/analyze-chrome-trace.py /tmp/boot.json --top 25
# (analyze-chrome-trace.py now recovers the truncated trace that a PASSING test
#  leaves — the FlushGuard is never dropped on clean exit.)

# CPU attribution if you need it:
#   scripts/analyze-samply-profile.py  (samply sampling profiler)
#   HOLON_PERF_FLAMEGRAPH=/tmp/fg cargo nextest … → folded stacks → speedscope
```

Per-transition floor is visible on the always-on `[inv-sql-budget] … wall=…` line
(run with `--no-capture`); note the composed path's `apply=/settle=/…` sub-timers
read 0 ms — that instrumentation is E2ESut-only; the composed path's per-transition
floor is the flat `SETTLE` sleep in `harness.rs::apply` (`wide_e2e.rs::SETTLE`).
