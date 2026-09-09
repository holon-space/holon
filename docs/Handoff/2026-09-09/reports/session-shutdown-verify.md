# Adversarial verification — lane `session-shutdown`

Verifier `pwd` for every command below: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/session-shutdown`

Tree identity asserted before every gate. `jj log -r @ -T 'commit_id.short(12)'` printed
`017d0e57b10d`; parent `xksmpmxy ec6ae4c7`. Both greps passed
(`SessionShutdown` in `crates/holon-api/src/lifecycle.rs`, `shutdown_session` in
`crates/holon-app/src/session.rs`). The gate script re-asserted both and echoed
`VERIFY TREE: …/session-shutdown` / `VERIFY REV: 017d0e57b10d`
(`lane-logs/verify-driver-41337.log:1-2`).

**Overall: CONFIRMED with defects.** The mechanism works and every gate I ran myself is
green. Three claims are overstated: the "never a silent detach" property, the "four quit
paths" count, and the completeness of the task-family enumeration.

---

## C1 — the primitive and the single teardown — CONFIRMED with two defects

The type is what the claim says. `crates/holon-api/src/lifecycle.rs:47-151`:
a `CancellationToken` plus `Mutex<Vec<(String, JoinHandle<()>)>>`; `shutdown()` cancels,
takes the registry, and `tokio::time::timeout_at(deadline, handle).await` per task,
collecting the names that miss the deadline into `ShutdownTimedOut` (lifecycle.rs:131-149).
There is no `.ok()`, no `let _ =` on a join result, and no path that returns `Ok` with a
named task still unjoined: `still_running.is_empty()` is the sole `Ok` gate
(lifecycle.rs:142). `spawn()` asserts the token is not already cancelled
(lifecycle.rs:79-83), so a post-shutdown registration is loud rather than orphaned.

`shutdown_session` (`crates/holon-app/src/session.rs:118-134`) propagates the
`ShutdownTimedOut` with `?` before it touches the actor, so the store cannot close on a
straggler.

### Defect 1 — `abort()` without join in three families (contradicts "never a silent detach")

Three sites stop their children with `abort()` and never join them:

- `crates/holon/src/api/action_watcher.rs:116-119` — pair watchers
- `crates/holon/src/api/holon_rule_watcher.rs:133-136` — rule watchers
- `crates/holon-frontend/src/reactive.rs:2494-2496` — `state.task.abort()` per UI watcher

`abort()` schedules cancellation; the task is dropped at its next yield point, on another
worker thread. The registered parent (`action-discovery`, `holon-rule-discovery`,
`ui-watchers`) is joined and `shutdown()` returns `Ok`, while the aborted children may
still be inside an in-flight store read. `shutdown_session` closes the actor immediately
after. So for these three families the guarantee is "abort and hope", not "join or report
by name" — exactly the shape the type's own doc comment (lifecycle.rs:28-30) says it exists
to prevent. The `ui-watchers` case is two hops deep: aborting `state.task` drops the
`WatchHandle`, whose `ActorAbortGuard` then aborts five more actors
(`crates/holon/src/api/ui_watcher.rs:71-87`, `:110-166`), none of them joined either.

Not observed as a failure: the green test allows 1500 ms of settling
(`session_shutdown_stops_watchers.rs:85`), which is far longer than the abort takes in
practice. It is a design gap, not a reproducible red.

### Defect 2 — the `BackendEngine` resolution failure is swallowed

`crates/holon-app/src/session.rs:127-133`:

```rust
match injector.try_resolve::<BackendEngine>() {
    Ok(engine) => engine.db_handle().shutdown().await…?,
    Err(e) => tracing::debug!("[shutdown] no BackendEngine to close: {e}"),
}
```

Any resolution failure — not only "this wiring has no Turso" — is downgraded to `debug!`
and `shutdown_session` still returns `Ok`. The store is then never closed and the caller
is told the teardown succeeded. This is the `_ => default` shape the repo `CLAUDE.md`
"Fail loud, never fake" and "Parse, don't validate" sections both call out. A no-Turso
wiring is distinguishable at registration time, so the branch does not need to be a
catch-all.

Minor: `frontends/gpui/src/main.rs:332` and `frontends/tui/src/main.rs:87` log the `Err`
with `tracing::error!` and continue. Acceptable on a quit path (it is reported, not
swallowed), but the process still exits 0 with watchers alive.

## C2 — the quit paths — REFUTED as stated (three of four), no old ordering left

`rg -n 'shutdown_session' crates frontends` returns exactly four sites:
the definition (`crates/holon-app/src/session.rs:118`), the re-export
(`crates/holon-app/src/lib.rs:77`), and **three** callers:

| Path | Site |
|---|---|
| gpui `main` | `frontends/gpui/src/main.rs:332` |
| TUI `main` | `frontends/tui/src/main.rs:87` |
| `TestEnvironment::stop_app` | `crates/holon-integration-tests/src/test_environment.rs:1021` |

The fourth, the keystone `reboot()`, **does not exist in this tree**. `rg -n 'fn reboot'`
finds only `boot_suite/junction_survives_reboot_repro.rs:507`, and `rg 'HOLON_PBT_REBOOT'`
over `crates` returns nothing. The change is a patch file
(`lane-logs/keystone-reboot-shutdown.patch`) for a lane whose base does not carry
`HeadlessFrontendComponent::reboot`. The lane report is honest about this
(its quit-path table says "see Handoff"); the claim as handed to me is not.

Consequence: nothing in the committed tree exercises `shutdown_session` in a reboot.
Only `stop_app` does, indirectly.

Old ordering: `rg --multiline 'db_handle\(\)\s*\.shutdown\(\)'` finds the call in exactly
three places — `session.rs:128`, and the two new tests, which call it deliberately. No
frontend or harness closes the actor on its own any more.

`ActorAbortGuard` is **not** dead code. It is still used by `loro_ui_watcher.rs:189`,
`advice_reconciler.rs:151`, `ui_watcher.rs:71`, and `WatchHandle`
(`crates/holon-api/src/streaming.rs:835`). Only the `ClockSchedulerHandle` and
`LoroSyncControllerHandle` fields were removed.

## C3 — eight families registered — CONFIRMED; the enumeration is INCOMPLETE

The eight names are registered and the test asserts every one of them before shutting down
(`crates/holon-app/tests/session_shutdown_stops_watchers.rs:47-63`). That assertion is the
right anti-vacuity guard and it is real: I saw the test pass in my own nextest run.

But "any spawned task NOT registered = finding" yields several. Enumerating every
`tokio::spawn` / `spawn_actor` in the named crates and filtering to production,
session-scoped, store-touching work:

**Finding 3a — `AdviceReconcilerHandle` is the clock-scheduler defect, unfixed.**
`crates/holon/src/sync/advice_reconciler.rs:154-160` spawns a task that **owns a
`DbHandle` and runs DDL** (`apply_plan(&db_handle, …)`), plus a CDC drainer at `:175`.
Both are held only by an `ActorAbortGuard` on `AdviceReconcilerHandle`
(`advice_reconciler.rs:47`), and that handle lives on the `BackendEngine`
(`crates/holon/src/api/backend_engine.rs:94`). That is the exact lifetime argument the lane
wrote into `clock_scheduler.rs:239-243` to justify moving the ticker: the engine is dropped
*after* the actor closes, so abort-on-drop comes too late. Same shape, same crate, not
registered, not disclosed.

**Finding 3b — `spawn_live_entity_refresh` is an unregistered polling loop.**
`crates/holon-loro-wiring/src/loro_ui_watcher.rs:99`, called from production wiring at
`:84`. It is an infinite loop that reads through a `BlockQuerySource`. It exits only when
its `Weak<ProfileResolver>` fails to upgrade, and the resolver lives on the engine — again
dropped after the actor closes.

**Finding 3c — `IntegrationProjector::reproject_on`.**
`crates/holon-app/src/integration_projection.rs:226`, driven from `:213` and `:215`.
Signal-driven, re-projects into the store, unregistered.

**Finding 3d — boot's `post_ready_work`.** `crates/holon-app/src/wiring.rs:759` detaches
the post-ready boot work when `wait_for_ready` is false. Bounded, but it reads the store
and is not joined by the shutdown.

Lower concern, listed for completeness: `crates/holon-api/src/live_data.rs:449`
(the CDC subscribe actor, ends when its source stream ends),
`crates/holon-orgmode/src/file_watcher.rs:168` (notify pump, no store reads),
`crates/holon/src/api/backend_engine.rs:870` and `:979` (stream-driven),
`crates/holon-frontend/src/memory_monitor.rs:58` (RSS sampling only),
`crates/holon-loro/src/debounced_commit_worker.rs:179` (Loro only).
`crates/holon-loro/src/loro_blocks_datasource.rs:122` `start_polling` is a 500 ms poll loop
over the store but has **no callers** — dead code, out of scope.

None of these are scoped out anywhere in the report or the code. 3a in particular is the
same defect the lane discovered, one file away.

## C4 — red for the right reason — CONFIRMED; the green test does NOT cover `shutdown_session`

The red is genuine. `lane-logs/RED-17088.log:139-182` shows the panic at
`session_shutdown_stops_watchers.rs:66` with the bugfunnel entry's signatures verbatim:
`[supervisor:org-writeback] DIED 4 times within 60s — GIVING UP` (line 164),
`[UiWatcher] render_entity('block:shutdown-probe-page') failed: … Actor channel closed`
(line 160), `[LoroSyncController] Outbound reconcile failed: … Actor channel closed`
(lines 179-181), `[OrgMode] Block change error … Actor channel closed` (lines 132, 172-173).

The anti-vacuity machinery is real, not decorative. `capture_errors`
(`crates/holon-app/tests/session_shutdown/harness.rs:51-69`) installs a **global**
subscriber and self-asserts it received one event before the test proceeds; the teeth
binary `session_shutdown_orphan_teeth.rs` runs the same boot in the wrong order and asserts
the log **is** noisy, so a boot with no live watchers cannot pass the silence assertion for
free. Both were green in my own run.

**Defect 4a — the green test bypasses the claimed single teardown.**
Answer to the brief's question: **no**, the test would still pass if `shutdown_session`
were a no-op. `session_shutdown_stops_watchers.rs:65-76` calls
`booted.shutdown.shutdown(DEFAULT_SHUTDOWN_TIMEOUT)` and then
`booted.engine.db_handle().shutdown()` directly. It never calls `holon_app::shutdown_session`.
It pins the primitive and the eight registrations; it does not pin the ordering inside the
function the whole lane is named for, nor any of the three quit paths that call it. With
the keystone reboot path absent (C2), `shutdown_session` itself is covered only indirectly,
by `TestEnvironment::stop_app` — where a no-op would surface as a WAL stall on the next
open rather than as a shutdown assertion.

## C5 — gates — CONFIRMED, all reproduced independently

Run by me via `bash ~/.claude/skills/orchestrator/scripts/with-build-slot.sh`, driver
`lane-logs/verify-driver-41337.log`.

| Gate | Runner summary line | Log |
|---|---|---|
| `cargo check --workspace --all-targets` | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 45.51s`; `V_CHECK_EXIT=0` | `lane-logs/verify-check-v41337.log` |
| nextest `-p holon-api -p holon-app -p holon -p holon-orgmode --features holon-orgmode/di` | `Summary [ 297.961s] 1451 tests run: 1445 passed (10 slow), 6 failed, 10 skipped`; `V_NEXTEST_EXIT=100` | `lane-logs/verify-nextest-v41337.log` |
| `just keystone-smoke` | `test result: ok. 4 passed; 0 failed`; `V_SMOKE_EXIT=0` | `lane-logs/verify-smoke-v41337.log` |
| `just hand-authored` | `test result: ok. 9 passed; 0 failed … finished in 1626.18s`; `V_HAND_EXIT=0` | `lane-logs/verify-hand-v41337.log` |
| `scripts/bugfunnel.py check` | `659 entries, 0 problems`; `V_BUGFUNNEL_EXIT=0` | run directly, and `lane-logs/verify-bugfunnel-v41337.log` |

The 6 nextest reds, all on the lane-rules known list:

- `holon::e2e_backend_engine_test` × 5 (`test_multiple_operations_sequence`,
  `test_create_and_delete_workflow`, `test_basic_query_execution`,
  `test_operation_triggers_stream_update`, `test_query_and_watch_stream`) — the
  "`e2e_backend_engine_test` matview reds" entry.
- `holon::turso_storage_repros tabs_main_panel_delivery::cursor_filtered_main_panel_delivers_at_vault_scale`
  — the "`cursor_filtered_main_panel`" entry.

`scripts/keystone-known-reds.sh lane-logs/verify-nextest-v41337.log` calls the last one
`[novel]` (`5.553415958s` against a "couple of seconds" budget). That is a classifier gap,
not a lane regression: the signature is on the lane-rules list by name, the other five fail
by error return rather than panic so the script does not see them at all, and the whole
suite is unrelated to task lifetime. **No novel red.**

My run did NOT reproduce the lane's one timeout
(`turso_block_query_source_round_trip_pbt`); it passed. That supports the lane's
load-flake reading.

Note: my nextest scope was wider than the lane's (`holon-api` and `holon-orgmode` added).
Both new binaries passed:
`PASS [ 13.532s] holon-app::session_shutdown_orphan_teeth …` and
`PASS [ 14.549s] holon-app::session_shutdown_stops_watchers …`, plus both
`holon-api lifecycle::tests::*` unit tests.

## C6 — dependency hygiene — CONFIRMED, clean

Added crate: **`tokio-util` 0.7**, `default-features = false`, declared once in the
workspace table (`Cargo.toml:86`) and consumed by `crates/holon-api/Cargo.toml:36`.

`Cargo.lock` changed by exactly **two lines**, both membership entries in existing package
dependency lists: `tokio-util` under `holon-api`, and `tracing-subscriber` under
`holon-app`. **No new `[[package]]` block was added**, which proves `tokio-util` was
already in the workspace graph (it is — `tokio-stream`, `iroh` and others pull it) and that
this brings in no new transitive tree. No version bumps anywhere in the lock, so
`cargo update` was not run.

The second addition, `tracing-subscriber` on `crates/holon-app/Cargo.toml:55`, is in the
`[dev-dependencies]` block and exists for the test harness's `ErrorLog` layer. Also already
in the graph.

## C7 — the remaining reboot-smoke red — does NOT touch shutdown code paths

Read from
`/private/tmp/claude-501/…/scratchpad/reboot-smoke/gate-out/reboot-smoke.log`.

Decisive line, `reboot-smoke.log:1047`:

```
ERROR holon_org_format::models: org render is DEGRADED for this block:
NO emission of this block settles; write-back may loop on it
block="block:bulk-0-0" rung="Unrepresentable"
```

And `:1061` carries every divergence, all on the **same block**:
`block:bulk-0-0: content: sut="[[]]" ref=""` across `org`, `loro`, `matview`, `block_raw`
and `sql`.

**Verdict: no.** The signature does not plausibly touch shutdown code. The emitting target
is `holon_org_format::models` (`models.rs:1309`), an org-rendering module with no task,
actor or lifetime involvement; the failing content is an empty link `[[]]` that the org
renderer cannot represent. Contrary to the report, these are almost certainly **one**
defect and not two: the block whose emission never settles is the same block whose content
round-trips as `[[]]` instead of `""`. An unrepresentable render is exactly what produces a
non-empty SUT content against an empty reference.

Independent corroboration for the lane's actual claim: `rg 'Actor channel closed'` over
that 598 KB log returns **nothing**, and `inv-no-observed-errors` reads `2/2` in every
engagement summary (`:1082`, `:1208`, `:1247`, `:1284`, `:2014`). The overall
`test result: FAILED. 3 passed; 1 failed` at `:6566` is attributable to the org divergence.

I did not run a population A/B, per instruction.

## C8 — secrets and file scope — CONFIRMED clean, one note

`jj diff -r 017d0e57b10d --git` matched against `ANTHROPIC|API_KEY|SECRET|PASSWORD|PRIVATE
KEY|holon-pkm|Bearer` returns **zero hits**. The three new shell scripts were read in full:
they export only `CARGO_TARGET_DIR`, `HOLON_PBT_REBOOT`, `HOLON_PBT_REBOOT_WEIGHT`,
`HOLON_PBT_FORCE_FULL` and `RUSTC_WRAPPER`. No vault content anywhere; the probe fixture in
`harness.rs:83-93` is synthetic.

Files outside `crates/`, `docs/`, `Cargo.*`:

- `frontends/gpui/src/main.rs`, `frontends/tui/src/main.rs`, `frontends/holon-worker/src/lib.rs`
  — legitimate quit-path and wiring changes.
- `scripts-lane/gate-final.sh`, `scripts-lane/gate-session-shutdown.sh`,
  `scripts-lane/reboot-smoke.sh` — lane scaffolding, tracked-added, so they will land.

The scaffolding follows existing precedent: `scripts-lane/` already holds 14 tracked files
at the parent `ec6ae4c7431a`, from earlier lanes on this chain. So this is accumulating
cruft on an established (questionable) convention, not a new violation. All three hardcode
absolute paths into this session's scratchpad, so they are inert for anyone else.

## C9 — comments — three history violations and one over-explained doc

Measured against `~/.claude/skills/commenting/SKILL.md` rule 1 (current state only, never
history — including "history in present-tense disguise") and rule 3 (at most two reasons).

| Site | Problem |
|---|---|
| `crates/holon-app/src/session.rs:108` | "Reversed — which is what every restart path **did before this existed** — …". Rule 1 verbatim. |
| `crates/holon/src/sync/clock_scheduler.rs:239-243` | "`ClockSchedulerHandle`'s `ActorAbortGuard` was not enough … so an abort-on-drop alone **let** the ticker reconcile against a dead store." Past-tense pre-fix behaviour. |
| `crates/holon-integration-tests/src/test_environment.rs:1005-1008` | "Closing the actor under them **left** every watcher reading a dead store until the org-writeback supervisor declared itself permanently degraded." Same. |
| `crates/holon-frontend/src/reactive.rs:2477-2484` | Three reasons in one doc comment: where the teardown belongs, that dropping a `WatcherState` would detach, and why `Weak`. Rule 3 allows two. |

Context that lowers the severity: history narration is endemic in this tree — a grep for
"used to" / "no longer" / "is retired" across the same crates returns well over a hundred
pre-existing hits. The new comments are in register with the codebase, just not with the
skill. Everything else I read is proportionate and explains a real non-obvious "why"
(the `biased` rationales in particular are exactly right).

Two `dead_code` warnings on the harness (`harness.rs:96`, the unused `session` and
`reactive` fields of `Booted`) appear in the nextest log. Cosmetic.

## Report-accuracy defects

Three log citations in `lane-report-session-shutdown.md` name files that do not exist:

| Report cites | Actually in `lane-logs/` |
|---|---|
| `gate-tests-97862.log` | `gate-tests-8158.log` |
| `gate-tests2-24628.log` | `gate-tests2-84474.log` |
| `isolate-timeout2-4229.log` | `isolate-timeout2-79358.log` |

The content behind each is what the report says (`test result: ok. 1 passed` twice in each
gate-tests log), so the claims stand; only the pointers are wrong. Also, the report's prose
says "all **seven** task names" (line 128) where the table and the test both list **eight**.

---

## Weave assessment

Safe to weave, after nav-latency. Nothing found is a correctness regression; every defect
is either a residual race with no observed failure, a completeness gap, or cosmetic.

The report's `## For the nav-latency lane's weave` section (lines 210-222) **is concrete
enough to apply at rebase**. It names the conflicting construct (nav-latency's
`LoroProjection` gains a bare `_task: JoinHandle` with no abort-on-drop), the exact site
this lane changes (`crates/holon-loro/src/loro_sync_controller.rs`, where the bare `_task`
field is deleted and the loop becomes `loro-outbound-reconcile`), and the two edits the
merged result needs: `shutdown.spawn("loro-projector", …)` with a `biased` cancel arm, plus
that name added to the assertion list in
`crates/holon-app/tests/session_shutdown_stops_watchers.rs:48-57`. The assertion list is the
mechanical safety net — if the rebaser forgets the hook but adds the name, the test reds;
if they forget both, the projector silently falls out, which is the one thing to watch for.

---

# Rev 2

Verifier `pwd` for every command: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/session-shutdown`.
`jj log -r @ -T 'commit_id.short(12)' --no-graph` printed **`ad0650ced03f`** (rev 1 was
`017d0e57b10d`). The gate script re-asserted the tree with two rev-2 markers and echoed
`VERIFY TREE: …/session-shutdown` / `VERIFY REV: ad0650ced03f`
(`lane-logs/v2-driver-9981.log:1-2`). Diff is now 39 files, +1137/-151.

**Verdict: CONFIRMED, with one refuted sub-claim and one factual error in the report.**
Every rev-1 defect I raised is genuinely fixed. Two claims are overstated.

## C3 — registration — CONFIRMED for the fixes, REFUTED as stated ("eleven, all asserted")

All four families I named in rev 1 are now registered, each with a `biased` cancel arm:

| Name | Site | Check |
|---|---|---|
| `advice-reconciler` | `crates/holon/src/sync/advice_reconciler.rs:145` | `AdviceReconcilerHandle` is now a unit marker (`:44`); the `_aborts: ActorAbortGuard` field is gone, so the engine-lifetime hazard is gone with it. |
| `advice-drainer` | same file, `:184` | Feeds the above; cancel arm present. |
| `integration-reprojector` | `crates/holon-app/src/integration_projection.rs:247` | `reproject_on` takes the shutdown; the `for_each` pump is raced against `cancelled`. |
| `loro-entity-refresh` | `crates/holon-loro-wiring/src/loro_ui_watcher.rs:99` | The `sleep(LORO_WATCH_POLL)` became a `biased` select against cancellation. |
| `boot-post-ready` | `crates/holon-app/src/wiring.rs:761-768` | The bare `tokio::spawn(post_ready_work)` is now `shutdown.spawn("boot-post-ready", …)`. |

### Defect R2-a — the count is wrong and three names are NOT asserted

`rg` over every `shutdown.spawn(…)` name yields **twelve** static names plus the dynamic
`supervisor:{component}` from `spawn_supervised` — thirteen families, not eleven.

The test's assertion list (`crates/holon-app/tests/session_shutdown_stops_watchers.rs:45-56`)
contains **ten**:

```
supervisor:org-writeback, org-writeback-consumer, file-sync-controller, ui-watchers,
loro-outbound-reconcile, action-discovery, holon-rule-discovery, clock-scheduler,
advice-reconciler, advice-drainer
```

Missing: **`integration-reprojector`, `loro-entity-refresh`, `boot-post-ready`.**
The report states at line 250 "All eleven names are in the test's assertion list, so a
future watcher that skips the seam reds rather than falling out silently." That sentence is
false, and the property it claims does not hold for those three.

At least two are *structurally* unassertable by this test, which is the more interesting
half:

- **`boot-post-ready`** is spawned only when `wait_for_ready == false`
  (`crates/holon-app/src/wiring.rs:761`). That flag is the production setting; the comment
  at `:695-698` says tests run `wait_for_ready=true` and await inline. So the one family
  that exists *only* in production is the one the harness can never observe.
- **`loro-entity-refresh`** is registered from `build_turso_free_profile_resolver`, reached
  only through `crates/holon-app/src/no_turso.rs`. The harness boots a Turso session.

`integration-reprojector` depends on `self.vm.signals()` being non-empty for the booted
configuration; I did not establish whether it fires in this boot.

So the anti-vacuity net covers ten of thirteen, and cannot be extended to at least two
without a second boot shape. That is a coverage gap to state plainly, not a blocker.

### Defect R2-b — the "none reads the storage actor" argument is wrong at one site

The brief told me not to accept the argument. Assessed per site:

| Site | Lane's claim | My finding |
|---|---|---|
| `crates/holon-api/src/live_data.rs:449` | no store read | **Holds.** A CDC pump with no `query` call; returns when its source stream ends. |
| `crates/holon-orgmode/src/file_watcher.rs:168` | no store read | **Holds.** Parked on `source_rx.recv()`; no store access. |
| `crates/holon-frontend/src/memory_monitor.rs:58` | no store read | **Holds.** RSS sampling only. |
| `crates/holon-loro/src/debounced_commit_worker.rs:179` | no store read | **Holds.** All three callers are in `crates/holon-loro/src/loro_share_backend.rs` (`:170`, `:308`, `:365`) — Loro/share work, no SQL. |
| `crates/holon/src/api/backend_engine.rs:979` | no store read | **Holds.** Emits initial rows, then pumps. |
| `crates/holon/src/api/backend_engine.rs:870` (`spawn_eager_stream`, `:828`) | no store read | **REFUTED.** |

`spawn_eager_stream`'s actor calls the store directly on every CDC wake:

```rust
let rows = match db_handle.query(&sql, HashMap::new()).await {
```

at `crates/holon/src/api/backend_engine.rs:898`, and on failure emits
`tracing::error!(relation = %relation, "[eager_requery] re-execution failed; …")` at `:906-914`.
It is unregistered and it reads the storage actor. The blanket statement at report line 253
is false.

The *conclusion* survives, but for a reason the report does not give: the loop's only wake
source is `cdc_rx.recv()` (`:889-893`), and it returns on `RecvError::Closed`. The CDC
broadcast sender dies with the storage actor, so the task exits rather than querying. Its
safety comes from its wake source, not from an absence of reads. That distinction matters
because the stated reason is what the next reader will rely on when adding a task here.
Consistent with this, the ERROR string never appeared: the harness test asserts log silence
and passed in my run.

## C1 — abort-then-join and the substrate branch — CONFIRMED

**Join.** All three families now `abort()` and then `await` the handle:
`crates/holon/src/api/action_watcher.rs:120-121`,
`crates/holon/src/api/holon_rule_watcher.rs:137-138`,
`crates/holon-frontend/src/reactive.rs:200-201` (`state.task.abort(); let _ = state.task.await;`).
The `let _ =` discards a `JoinError`, which is correct rather than a swallow: a deliberately
aborted task always returns `Err(Cancelled)`, so there is no error to report. The parent no
longer returns before its children have stopped.

**Substrate branch.** `open_and_register_core` registers `StorageSelector` as a value
(`crates/holon/src/di/lifecycle.rs:81-84`), and `shutdown_session`
(`crates/holon-app/src/session.rs:118-140`) matches on it exhaustively:
`LoroMemory => Ok(())`, `Turso =>` resolve the engine or return an `Err` naming the failure.
No catch-all, no `.ok()`.

I checked the branch cannot panic on a container that never ran `open_and_register_core`.
Every production container does: `build_no_turso_container` calls it with `LoroMemory`
(`lifecycle.rs:183`), the Turso paths at `:139` and `:161`, plus `frontends/tui/src/di.rs:46`
and `frontends/mcp/src/main.rs:431`. The three bare `Injector::root()` calls in
`frontends/gpui/src/di.rs` (`:182`, `:195`, `:207`) and the one in
`crates/holon-app/src/boot_error.rs:198` are all inside `#[cfg(test)] mod tests`
(guard at `boot_error.rs:174-175`); gpui production resolves through `app.injector()`.

`rg 'debug!|\.ok\(\)|let _ = '` over `session.rs`, `lifecycle.rs` and both frontend `main.rs`
files returns **no hits in either shutdown file**. Every gpui/tui hit is pre-existing and
unrelated (logging init, env vars, debug slots), each already carrying an `ALLOW(ok)` marker.

### Residual (unchanged severity, narrower than rev 1)

`ui-watchers` joins `state.task`, whose drop runs `ActorAbortGuard::drop`
(`crates/holon-api/src/streaming.rs:814-822`) — a `for h in &self.handles { h.abort(); }`
with no join, because `Drop` cannot await. So the five inner `watch_ui` actors
(`crates/holon/src/api/ui_watcher.rs:71-87`, `:110-166`) are still aborted without being
joined. One hop short of the stated guarantee. The aborts are issued while the store is
still open, so the exposure is small, and the test's silence assertion did not catch
anything. Worth knowing, not worth blocking.

## C4 — the second hole — CONFIRMED, red reproduced from the log

The test now drives the production entry:
`holon_app::shutdown_session(&booted.injector)` at
`crates/holon-app/tests/session_shutdown_stops_watchers.rs:66`, with `Booted.injector`
added to the harness (`session_shutdown/harness.rs:100`, populated at `:162`/`:180`).

Both halves are asserted, and reasoning from the assertions each is load-bearing alone:

1. **Store closed** (`:74-83`): `db_handle().query("SELECT 1", …)` must return `Err`. A
   teardown that stopped the tasks but skipped the actor close reds here.
2. **Watchers silent** (`:94-99`): the captured ERROR log must be empty. A teardown that
   closed the actor but skipped the task shutdown reds here — and the sibling teeth binary
   proves that path *is* noisy, so this is not vacuous.
3. A fully stubbed `Ok(())` reds at (1) first.

The lane's red confirms (3) exactly. `lane-logs/C4-RED2-10532.log:109-110`:

```
panicked at crates/holon-app/tests/session_shutdown_stops_watchers.rs:79:5:
the storage actor is still serving reads after the teardown returned Ok — the second half
of the shutdown ordering did not happen
```

with `test result: FAILED. 0 passed; 1 failed` at `:10`. Restore proof: the `session.rs`
in the tree I verified carries the real implementation, not the stub — I read the full
function body above, which is stronger evidence than a hash of it.

Both binaries passed in my own run (below), so the green half is mine, not the lane's.

## C2 / C9 / hygiene — CONFIRMED

**Quit paths.** The report's table (lines 50-55) now records three callers and marks the
keystone `Reboot` row "NOT in this tree", with the patch handed to the other lane. That
matches what I found in rev 1 and re-checked here.

**`scripts-lane/`.** `jj diff -r @ --stat | grep -c 'scripts-lane'` returns **0**. The
directory still holds 14 files, but `jj file list` returns 14 at both `@` and `@-`, so this
lane adds none. The three scripts moved to `lane-logs/scripts/` (untracked):
`gate-final.sh`, `gate-rev2.sh`, `gate-session-shutdown.sh`, `reboot-smoke.sh`.

**Log citations.** Every `lane-logs/…` path in the report now resolves on disk; the three
rev-1 mismatches are fixed.

**Comments.** All three rev-1 rule-1 violations are rewritten to present tense:
`crates/holon-app/src/session.rs:105-107` ("A watcher still running when the actor closes
reads a dead store…"), `crates/holon/src/sync/clock_scheduler.rs:240-242`
("abort-on-drop is too late to keep the ticker off a dead store"), and
`crates/holon-integration-tests/src/test_environment.rs:1005-1006`. The rule-3 violation at
`crates/holon-frontend/src/reactive.rs` is down to two reasons. Grepping the added comment
lines for history markers leaves three mild "no longer" residuals —
`clock_scheduler.rs:79` ("this handle no longer owns its lifetime") and the same sentence in
both test files — which read as history-in-disguise to someone without the diff. Minor.

## C5 — gates, all re-run by me

Driver `lane-logs/v2-driver-9981.log`, semaphore-wrapped.

| Gate | Runner summary | Exit |
|---|---|---|
| `cargo check --workspace --all-targets` | `Finished \`dev\` profile [unoptimized + debuginfo] target(s) in 35.52s` | `V2_CHECK_EXIT=0` |
| nextest `-p holon-api -p holon-app -p holon -p holon-orgmode --features holon-orgmode/di` | `Summary [ 495.100s] 1451 tests run: 1443 passed (8 slow), 6 failed, 2 timed out, 10 skipped` | `V2_NEXTEST_EXIT=100` |
| `just keystone-smoke` | `test result: ok. 4 passed; 0 failed … finished in 118.26s` | `V2_SMOKE_EXIT=0` |
| `just hand-authored` | `test result: ok. 9 passed; 0 failed … finished in 1069.56s` | `V2_HAND_EXIT=0` |
| `scripts/bugfunnel.py check` | `659 entries, 0 problems` | `V2_BUGFUNNEL_EXIT=0` |

Both new binaries pass:
`PASS [ 17.444s] holon-app::session_shutdown_stops_watchers …` and
`PASS [ 17.627s] holon-app::session_shutdown_orphan_teeth …`, plus both
`holon-api lifecycle::tests::*`.

`hand-authored` was **9/9 on the first run** — I did not reproduce the lane's
`inv-main-panel-rows-match-focus` DROPPED ROW red.

### The 8 failures, all registered in `docs/Testing/KeystoneKnownReds.md`

- 5 × `holon::e2e_backend_engine_test` matview reds — registered at `KeystoneKnownReds.md:314`.
- `cursor_filtered_main_panel_delivers_at_vault_scale` — row `vault-scale-main-panel-delivery`
  at `:351`, "load-sensitive: 853ms isolated, 7.5s contended".
- 2 × `holon::turso_block_query_source_round_trip_pbt` TIMEOUT — row
  `turso-block-query-source-round-trip` at `:350`, "load-sensitive under the parallel
  `-p holon -p holon-app` leg (D64.a); 21s isolated".

I isolated the timeout family myself rather than accepting the registry:
`cargo nextest run -p holon --features test-helpers --test turso_block_query_source_round_trip_pbt`
gave `Summary [ 38.501s] 2 tests run: 2 passed (2 slow), 0 skipped`
(`lane-logs/v2-isolate-tmo2-7741.log`) — 38.5s against the 120s cap, a 3× margin.
Load, not regression. Corroborating: this whole run was far more contended than my rev-1
run (nextest 495s vs 298s, keystone-smoke 118s vs 1.41s), which is why rev 2 shows the
timeouts and rev 1 did not.

**No novel red.**

## Open gap, carried forward

The reboot-weighted keystone smoke (`HOLON_PBT_REBOOT=1 HOLON_PBT_REBOOT_WEIGHT=40
HOLON_PBT_FORCE_FULL=1`) was **not** re-run against rev 2 by the lane, and not by me per
the brief. Rev 2 changed the teardown's own control flow — the `StorageSelector` branch — and
that scratch run is the only evidence the reboot transition is clean. The rev-1 result
(`0` occurrences of `Actor channel closed`, `inv-no-observed-errors` `2/2`) does not
automatically transfer. Also unchanged from rev 1: the `[[]]` / `NO emission … settles`
divergence on `block:bulk-0-0` is unrelated to shutdown and still open.

## Rev 2 weave assessment

Safe to weave, after nav-latency. Nothing found is a correctness regression. The three
defects are: an assertion list that covers ten of thirteen families (two structurally
unreachable from this test), a wrong justification for one unregistered store-reading task
whose safety happens to hold for another reason, and the one-hop-short abort of the inner
`watch_ui` actors.

The `## For the nav-latency lane's weave` section still applies unchanged, and the
assertion list remains the mechanical safety net at rebase: add `loro-projector` to
`crates/holon-app/tests/session_shutdown_stops_watchers.rs:45-56` together with the
`shutdown.spawn` hook, or the projector falls out silently the way `boot-post-ready`
currently does.
