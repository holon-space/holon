# Matview Lifecycle Ownership in the Database Actor (task #35)

Design rev 2, ratified. This document is the reference for the implementation;
Increments 1 and 2 are in scope for the first lane, 3 and 4 follow later.

## Premises (verified)

- `turso.rs` `run_actor` holds all state as locals passed `&mut` into
  `process_actor_command` (one `DbCommand` at a time).
- `handle_ddl` + deferred-DDL (`PendingDdl` + parked oneshots, released by
  `MarkAvailable`) already run on the actor.
- Matview `CREATE` already executes on the actor via `execute_ddl_with_deps`.
- FIVE `MatviewManager` construction sites: `di/registration.rs:177`,
  `di/registration.rs:556`, `api/backend_engine.rs:109`,
  `api/operation_dispatcher.rs:1062`, `holon-app/src/turso_seams.rs:501` (test
  seam) — each with its own `ddl_mutex` over the same `DbHandle`.

## 1. New `DbCommand` variants

- `AcquireViewLease { view_name, select_sql, requires: Vec<Resource>, response:
  oneshot<Result<LeaseGrant>> }` with `LeaseGrant { lease_id: u64, generation:
  u64 }`.
  Actor: `Live` → count+1, reply. Absent → create inline (cleanup orphaned DBSP
  state on conn, `CREATE MATERIALIZED VIEW IF NOT EXISTS` via the existing
  deferred-DDL path, `DDL_MATVIEW` priority), set `Live{count:1}`, reply.
  `Creating` → park response as waiter; the pending-DDL completion flips state
  and replies to all waiters. `requires` parsed CALLER-side
  (`parse_sql`/`extract_table_refs`) so parse failures fail loud before send.
- `ReleaseViewLease { view_name, lease_id, generation }` — one-way, no response.
  Decrement; on zero and not pinned: reap inline (dependents per F3 rule, `DROP
  VIEW IF EXISTS`, disclose DBSP residue, remove entry — which prunes the SQL).
  Stale generation (post-reset) silently OK; unknown view at current generation
  = ERROR log (guard bookkeeping bug).
- `EnsurePinnedView { view_name, select_sql, response }` — `preload()` path:
  create if absent, pinned FOREVER (even after later watch+release cycles).
- `ResetWatchViews { response: oneshot<Result<usize>> }` — boot
  `drop_stale_views` + `full_sync`: drop every `watch_view_%` in
  `sqlite_master`, clear map, bump generation.
- No stats command: actor updates a shared `Arc<MatviewStats>` (atomics:
  `leased_views`, `active_leases`, `pinned`) exposed via `DbHandle`.

## 2. Actor-owned state

Locals in `run_actor` (fold the growing param list into an `ActorState`
struct): `views: HashMap<String, ViewState>` with

```
ViewState = Live { leases: u32, pinned: bool }
          | Creating { waiters: Vec<oneshot<Result<LeaseGrant>>> }
```

plus `generation: u64` and the stats atomics.

DELETED WHOLESALE (when their last consumer goes — some deletions land in
Inc 3): `MatviewLeaseRegistry`, `REGISTRIES` static + `DatabaseId` keying,
`reap_mutex`, reaping marker set, `reap_pending`, `forget_if_unleased`, reaper
task + release channel, `known_views` cache + both `view_exists` probes,
`view_sql` map, `rematerialize_if_reaped`, all five `ddl_mutex`es and
`MatviewManager::new`'s mutex parameter.

STAYS OUTSIDE: demux task + eager `Unsubscribe` (c2; routing not lifecycle),
FDW priming (idempotent, ordinary `Query`/`Execute` before the command), the
stream-carried guard, `reconcile_named_view`.

## 3. Defect dissolution (contract the code must honor)

- **F1** — lease/mid-drop/pending-reap are actor-map fields mutated only
  between commands; a reap is one uninterrupted command execution;
  re-subscribe-during-reap is deterministic queue order (Release completes
  reap, Acquire recreates).
- **F2** — all matview DDL is actor command processing; no external mutex.
- **F3** — reap inline during release; a dependent with live leases refuses the
  reap LOUDLY; the only serialization is the command queue every query already
  traverses.

## 4. Round-trip budget

- subscribe = 1 actor round trip (+ in-process demux ack + existing
  initial-data query)
- release = 0 (one-way `try_send`; on `Full` spawn a task to `send().await` so
  the release is never lost, disclosed at WARN; on `Closed` a debug log)
- reap = 0 dedicated

## Increment 1 (holon-turso only, unused by prod)

The `DbCommand` variants, `ActorState`/view map, conn-based ports of
`drop_dependent_views` and `cleanup_orphaned_dbsp_state` (today they take
`DbHandle` — calling them from inside the actor SELF-DEADLOCKS; the conn port
is mandatory). The "database is locked" retry-loop deletion (fail-loud) goes in
ITS OWN ISOLATED COMMIT within this increment (bisect separability).

Gate: new unit tests driving `DbHandle` acquire/release/reset directly;
`cargo nextest run -p holon-turso` (tee to log).

## Increment 2 (lease-carried streams end-to-end; reaping turns ON here)

Port from the HELD workspace `.claude/worktrees/agent-a3b097788f41e034d`
(commit `2cbb27ad` — READ-ONLY donor): `RowChangeStream` struct + guard
(retarget the drop at `ReleaseViewLease`), `DemuxCommand::Unsubscribe` +
`subscriber_id` (c2), `WATCH_VIEW_PREFIX`, memstats wiring, AND the wrapper-hop
teardown fixes — the `tx.closed()` select-loops in `backend_engine.rs`
(`prepend_initial_data` forward loop) and `ui_watcher.rs`
(`forward_data_stream`, `enrich_stream`). Rewire `MatviewManager::watch` /
`subscribe_cdc` onto `AcquireViewLease`.

Adapt the held acceptance tests mechanically (constructor signatures;
`subscribe_cdc` gains a SQL argument; `watch_layer_counters` polls lease
metrics; REPLACE `reaper_releases_its_registry_once_the_manager_and_leases_are_gone`
— it pins the dead implementation — with "actor matview map empty + no reaper
task after all leases drop").

Gate, all three:
1. adapted `matview_lease_lifecycle.rs` green;
2. a NEW prod-wiring release test: `backend_engine` watch → consume initial
   batch → drop the stream → view actually reaped (assert via `sqlite_master` +
   stats);
3. keystone A/B: with reaping enabled the keystone must exit rc=0 with ZERO
   `holon_rule_watcher` errors (the held build's signature was rc=101/41 errors
   — that must be gone).

Keystone runs ONLY via the semaphore:
`/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-keystone -j1
--fg -- bash <script-file>` where the script cds to the lane worktree, asserts
the tree (`Cargo.toml` + `justfile` exist; `$PWD` is not the repo root), runs
`just keystone-smoke`, tees, and greps a positive test count. Builds: semaphore
id `holon-build` `-j4`, script files only, NEVER inline-quoted pipelines into
`parallel`.
