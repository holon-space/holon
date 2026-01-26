---
date: "2026-07-06"
session: "fable-review-turso-matview"
project: "holon"
type: review
scope: "crates/holon-turso matview layer (IVM, CDC, reconcile, preload)"
---

# Deep Review: Turso IVM Matview Layer

Reviewer stance: senior DB/IVM engineer, read-only. All paths absolute-relative to repo root.

## 1. Map of the matview layer

### 1.1 Named (boot-reconciled) matviews

Created via `reconcile_named_view` (crates/holon-turso/src/matview_manager.rs:48-144)
from `SchemaModule::ensure_schema` impls in crates/holon-turso/src/schema_modules.rs:

| Matview | SELECT source | Chain depth | Created at |
|---|---|---|---|
| `block` | `block_raw` LEFT JOIN `block_tags` LEFT JOIN `block_requires` + json_group_array + 16-col GROUP BY (sql/schema/block_matview.sql) | 1 | schema_modules.rs:183 |
| `block_requirement_edges` | `block_requires` JOIN **`block` matview** (sql/schema/block_requirement_edges_matview.sql) | 2 (matview-on-matview) | schema_modules.rs:227 |
| `block_with_path` | recursive CTE over **`block` matview** (sql/schema/blocks_with_paths.sql) | 2 | schema_modules.rs:263 |
| `current_focus` | `navigation_cursor` JOIN `navigation_history` (sql/schema/matview_current_focus.sql) | 1 | schema_modules.rs:324-338 |
| `focus_roots` | `navigation_history` WHERE closed_at IS NULL AND block_id IS NOT NULL (sql/schema/matview_focus_roots.sql) | 1 (no longer joins block) | schema_modules.rs:324-338 |

Ordering between modules is FluxDI `DbReady<R>` resource deps (`provides`/`requires` in each module;
NavigationSchemaModule requires `block` at schema_modules.rs:302-305 even though focus_roots no longer joins it — harmless over-declaration).

MCP integration sidecars also reconcile named views at runtime: crates/holon-mcp-client/src/mcp_integration.rs:580.

### 1.2 Dynamic watch views

Every reactive query becomes `watch_view_{hash(sql)}` via `MatviewManager::ensure_view`
(matview_manager.rs:419-505), called from:
- `BackendEngine::watch_query` / `query_and_watch` (crates/holon/src/api/backend_engine.rs:444-501) — params + context inlined via `inline_parameters` (literal substitution, backend_engine.rs:296-341) because matview DDL cannot hold bind params;
- `subscribe_sql` (backend_engine.rs:141-152).

GQL/PRQL compile to SQL `FROM block` (the matview) → **every watch view is a chained matview (depth 2)**.
`from descendants` expands to `FROM block_with_path` (backend_engine.rs:22-29) → **depth 3**.

Dependency graph:

```
block_raw ──► block ──► block_with_path ──► watch_view_* (descendants queries)
  block_tags ─┤   ├──► block_requirement_edges
  block_requires┘ └──► watch_view_* (most GQL/PRQL watches)
navigation_cursor+history ──► current_focus
navigation_history ──► focus_roots
```

### 1.3 CDC wiring

- Turso IVM fires CDC **only for matviews** (docs/Architecture/Schema.md:21-25); `relation_name` = view name.
- `TursoBackend::new` registers the change callback (crates/holon-turso/src/turso.rs:1177-1210) →
  `process_cdc_event` (turso.rs:1429-1530): parses rows, injects `data["_rowid"]`, derives entity id
  (`id` column, else **rowid fallback**), runs `coalesce_row_changes` (turso.rs:888-965:
  DELETE+INSERT→UPDATE, INSERT+DELETE→no-op), stamps a process-monotonic `seq` for the
  `cdc_emitted_watermark` settle primitive (turso.rs:602-613).
- Batches go to a `broadcast::channel(1024)` (crates/holon/src/di/lifecycle.rs:81) shared by all `DbHandle` clones.
- `MatviewManager::spawn_demux` (matview_manager.rs:274-370) is the single fan-out task: routes by
  `relation_name` to per-view mpsc(1024) subscribers. Backpressure policy is correct fail-loud: a full
  subscriber gets its stream **closed** (matview_manager.rs:332-345) forcing re-watch, never a dropped delta.

### 1.4 reconcile_named_view (boot path)

matview_manager.rs:48-144: probe `sqlite_master` for `type=view`, compare via `normalize_view_sql`
(whitespace collapse + lowercase + `" ("` strip, :29-36); unchanged → no-op; changed → `DROP VIEW` + recreate.
Crash recovery: `cleanup_orphaned_dbsp_state` drops `__turso_internal_dbsp_state_v*_{view}` (:152-171),
`CREATE ... IF NOT EXISTS`, and on residual "already exists" drops view+backing table and retries ONCE,
failing loudly instead of boot-looping (:109-141). Good: base tables never touched; tested at :764-829.

### 1.5 Preload path

`preload_startup_views` (crates/holon/src/di/lifecycle.rs:25-58) compiles `STARTUP_QUERIES` (PRQL,
crates/holon/src/di/mod.rs:76-…) → `BackendEngine::preload_views` (backend_engine.rs:184-198) which calls
`inline_parameters(sql, &HashMap::new())` — **an empty param map** — then `MatviewManager::preload`
(matview_manager.rs:511-577): 3 attempts, retry only on lock/schema-changed, and on final failure
**logs a warning and returns Ok(view_name)** (lazy creation later via watch_query is the safety net).

## 2. Hang / freeze hazards

### 2.1 Matview-on-matview is not an edge case — it is the architecture

Despite the `turso-chained-matview-hang` skill (dated 2025-01-24, pre-fork-fix) saying chained matviews
are unsupported and hang: production **relies on them everywhere** — `block_requirement_edges` (depth 2),
`block_with_path` (depth 2), and every `watch_view_*` (depth 2; depth 3 for `descendants`).
docs/Architecture/Schema.md:21-25 declares this deliberate, validated by the chained-matview preflight
(`bigdata/turso/bugs/holon_block_hydration_repro.sql`) on the pinned nightscape@holon Turso fork.

So the answer to "is matview-on-matview relied on despite the hang?" is **yes, pervasively** — the hang
skill is stale as a blanket statement, but the hang is NOT fully dead: the 2026-07-05 dogfood page-click
freeze was a `CREATE MATERIALIZED VIEW watch_view_*` (chained on `block`) hanging inside the DB actor.

### 2.2 The runtime-DDL freeze mechanism (root-caused, guarded, not eliminated)

- The DB actor executes commands **sequentially**; an unbounded `conn.execute(DDL)` parks the whole actor —
  every later query queues forever, app-wide freeze with no error. Comment + guard: `handle_ddl`,
  crates/holon-turso/src/turso.rs:1888-1933.
- Fix in place: `DDL_EXECUTION_TIMEOUT` (30s, env-overridable `HOLON_DDL_TIMEOUT_MS`) inside the actor
  (turso.rs:1935-1953). Caller-side `DEPENDENCY_TIMEOUT` (120s, turso.rs:474/528) provably did NOT help —
  it abandoned the caller while the actor stayed parked.
- Residual exposure: (a) the wasm32 branch has **no timeout** (turso.rs:1924-1930) — a chained-matview hang
  on the web worker still freezes it permanently; (b) a hang now costs a 30s app-wide stall per occurrence
  (all DB traffic queued behind it), then surfaces as a failed render — degraded, visible, but rough.

### 2.3 Where DDL happens at runtime (the triage suspects, confirmed)

1. `ensure_view` on first watch of any new query hash — i.e. **every navigation to a block whose query
   was never watched before** issues `CREATE MATERIALIZED VIEW` mid-session (matview_manager.rs:496,
   via backend_engine.rs:453/500). `DatabasePhase` doc explicitly allows DDL in ALL phases (turso.rs:67-71).
2. `drop_stale_views` at boot and on `full_sync` (matview_manager.rs:377-404, backend_engine.rs:125).
3. `reconcile_named_view` at boot per schema module, and at MCP-integration attach
   (mcp_integration.rs:580) — DDL while the app is live.
4. Orphaned-DBSP cleanup DROP TABLEs on those paths.

Each of these serializes through the single actor; a slow/hung one = the freeze. The 30s bound makes it
recoverable, not painless.

## 3. Stale-row / ghost-row family — root causes

### 3.1 Deletes are NOT reliably cascaded through chained views (confirmed upstream bug)

In-repo reproducer: crates/holon/examples/turso_ivm_chained_matview_stale_rows.rs — when MV-A’s source is
UPDATEd (join key change), MV-A updates but **MV-B (matview over MV-A) retains rows from MV-A’s previous
state**; raw SQL re-evaluation disagrees with the chained matview. Production sighting recorded in the
repro header (focus_roots kept 2 rows from the previously-focused doc). Since every sidebar/panel watch
is a matview chained on `block` (⋈ `block_tags` for the pages sidebar), a page delete that IVM
mis-propagates leaves the deleted page in the watch view = **stale sidebar**. This is the H1 of the
dogfood triage and it is an upstream Turso IVM correctness bug, only partially mitigated by fork pins
(the 290fbb4ff pin fixed the recursive-CTE UPDATE surface — see turso.rs:967-975 — not the general
chained-delete case).

### 3.2 Junction-table orphans (H2): declared FK CASCADE is dead code

`block_tags`/`block_requires` declare `ON DELETE CASCADE` (sql/schema/block_tags.sql, block_requires.sql)
but Turso does not enforce FKs, and `SqlOperationProvider::prepare_delete`
(crates/holon/src/core/sql_operation_provider.rs:726-791) deletes **only from `self.table_name`**
(block_raw) for the whole descendant cascade — junction rows are never deleted. Consequences:
- `block_requirement_edges` (JOIN on `block_requires`) depends on IVM correctly retracting the join when
  the `block` side vanishes — the exact chained-delete path that 3.1 shows is flaky;
- a re-created block with a recycled id silently inherits ghost tags/requires;
- permanent junction garbage growth.

### 3.3 Rowid-keyed CDC deletes no-op in the frontend row store (H3)

`process_cdc_event` Delete: entity id = row’s `id` column, **falling back to Turso rowid** when the view
has no `id` column or the record is unparseable (turso.rs:1481-1510). `MatviewManager::query_view`
deliberately aliases `rowid AS _rowid` so `LiveData` can build a rowid→user-key map (matview_manager.rs:579-596).
But `ReactiveRenderedRows`-style consumers key rows by `entity_uri_from_id_str(id)` and their Deleted arm
(crates/holon-frontend/src/reactive.rs:491-495) does a plain `remove` with **no rowid fallback map** — a
delete keyed by rowid removes nothing. Result: UI ghost row until a full re-render (`retain_keys`,
reactive.rs:514+, only runs on re-render snapshots).

### 3.4 The boot-seeded ghost row

`build_default_layout_blocks` / seed path (crates/holon-frontend/src/lib.rs:493-543) inserts the
`block:__default__` root-layout page + fixed page shells (`block:journals`, …) **by raw SQL, bypassing
OperationProviders/events/undo** ("bootstrap operation", lib.rs comment above :503). Three ghost enablers:
1. Visibility is only suppressed by a query filter `b.id != block:__default__` in the bundled
   `index.org` (lib.rs:486-489) — any watch/query missing that filter renders the seed row.
2. `LoroProjection` **withholds deletes until armed** specifically so these SQL-only seed rows are not
   deleted before Loro has adopted them (crates/holon/src/sync/loro_module.rs:227-235, `projection.arm()`) —
   a boot ordering window in which seeded rows are undeletable by design.
3. Cleanup of superseded seed blocks happens "next startup" (lib.rs:498-500), not when the real layout
   arrives — a whole session can show the seeded layout row alongside the real one.
Combined with 3.1/3.3, a seed row whose delete is mis-propagated or rowid-keyed becomes the classic
"ghost row that survives everything except an app restart".

### 3.5 Named matviews are never verified or rebuilt

`drop_stale_views` only drops `watch_view_%` (matview_manager.rs:381). `block`, `block_with_path`,
`block_requirement_edges`, `current_focus`, `focus_roots` persist across restarts, and
`reconcile_named_view` skips them when the SQL text is unchanged. There is a **known drift regression**
(sql/regressions/2026-05-13-ivm-block-matview-drift-on-mcp-startup.sql): once a named matview’s DBSP
state drifts from base tables, nothing ever repairs it — drift is immortal until the SELECT text changes.

## 4. IVM assumptions audit + prioritized fixes

Assumptions the code makes of Turso IVM, with verdicts:
- A1 "chained matview CREATE completes" — mostly true on fork, still hangs sometimes → guarded (30s), not on wasm.
- A2 "deletes/updates propagate correctly through chains" — **false in at least one repro’d shape** (3.1).
- A3 "CDC delete carries an identity consumers can key on" — false for id-less views unless the consumer keeps a rowid map (3.3).
- A4 "FK CASCADE cleans junctions" — false; unenforced (3.2).
- A5 "matview state survives restart consistently" — known-violated once (drift regression), unrepaired (3.5).
- A6 "no CDC batch is ever silently lost" — violated by design at two lag points, see P1-c.

### Prioritized fixes

- **P0-a Stale sidebar / chained deletes**: land the keystone page-delete transition + sidebar RefWatch
  (gap recorded in dogfood triage) so the ONE PBT reproduces 3.1; then either upstream-fix the chained
  delete retraction in the fork or add a settle-time consistency backstop: after `cdc_emitted_watermark`
  quiesce, re-run the view’s SELECT and diff against matview rows for actively-watched views (visible
  degraded-mode repair, logged). Files: crates/holon/examples/turso_ivm_chained_matview_stale_rows.rs
  (repro), matview_manager.rs:627-636 (watch), reference: wrapup stale-row detection net.
- **P0-b Rowid delete fallback**: give `ReactiveRenderedRows::apply` (holon-frontend/src/reactive.rs:491-495)
  the same `_rowid → key` map `LiveData` maintains (initial rows already carry `_rowid`,
  matview_manager.rs:591). Without it every id-less watch view leaks ghosts on delete.
- **P1-a Junction cleanup in prepare_delete** (sql_operation_provider.rs:726-791): emit
  `DELETE FROM block_tags/block_requires WHERE block_id IN (…)` (+ `required_id`) alongside block deletes;
  FK CASCADE is decorative under Turso.
- **P1-b Context-param preload is structurally dead**: `preload_views` inlines with an empty map
  (backend_engine.rs:193-195) so any startup query using `$context_id`/`from children/descendants` reaches
  `CREATE MATERIALIZED VIEW` with a raw `$var` → Turso "Variable" error → 3 futile retries → warn-and-Ok
  (matview_manager.rs:568-576). Fail-loud policy says: detect residual `$` placeholders BEFORE issuing DDL
  and skip with a single structured log (or preload against `QueryContext::root`). Matches the
  `turso-ivm-context-param-preload` skill.
- **P1-c CDC lag = silent corruption of ALL subscribers**: demux treats `broadcast RecvError::Lagged(n)`
  as warn-and-continue (matview_manager.rs:354-359) — n batches are gone, every subscriber’s incremental
  state is now wrong, yet only the *per-subscriber full-queue* case closes streams. On lag, close all
  subscriber channels (force re-watch) exactly like the full-queue path. Same for `DbHandle::row_changes`
  (turso.rs:629-636).
- **P1-d Named-matview drift repair** (3.5): boot-time cheap check (COUNT + checksum of matview vs its
  SELECT) per named view; on mismatch DROP+recreate and log loudly. Hook: reconcile_named_view’s
  unchanged branch (matview_manager.rs:65-73).
- **P2-a wasm DDL unbounded** (turso.rs:1924-1930): apply the same timeout via a wasm-compatible timer,
  or at least a watchdog log.
- **P2-b preload bypasses ddl_mutex + dep tracking**: preload uses bare `execute_ddl`
  (matview_manager.rs:540) while ensure_view uses `execute_ddl_with_deps` under `ddl_mutex`
  (matview_manager.rs:448,496-498). IF NOT EXISTS makes races benign, but preload can fire before deps
  are marked available → guaranteed first-attempt failures that the retry loop papers over.
- **P2-c prime_fdw_caches naive substring replace** (matview_manager.rs:676): `sql.replace(table, table_fdw)`
  corrupts SQL when one registered table name is a substring of another identifier ("block" →
  "block_fdw_tags"). Use the parsed AST it already has.
- **P2-d view_exists swallows errors as false** (matview_manager.rs:644-647): a transient query error
  routes into the CREATE path; IF NOT EXISTS saves correctness, but this violates the repo’s fail-loud rule
  and can mask real DB trouble.
- **P3 normalize_view_sql lowercases string literals** (matview_manager.rs:29-36): SELECTs differing only
  inside quoted literals (case, or embedded runs of spaces) mis-compare in both directions → a missed or
  spurious recreate. Tokenize-aware compare, or at minimum skip quoted spans.
- **P3 identifier interpolation**: view names/ids are format!-ed into SQL throughout the manager;
  internal-only today, but one MCP-supplied view name (mcp_integration.rs:580) reaches
  `reconcile_named_view` — validate `[A-Za-z_][A-Za-z0-9_]*`.

## Verdicts on the three headline questions

1. **Hang**: matview-on-matview is load-bearing across the whole read path; the hang is guarded (30s actor
   timeout, turso.rs:1943) not fixed, and unguarded on wasm. Runtime DDL is by design on every new watch.
2. **Stale sidebar**: a compound of upstream chained-delete IVM bug (repro in-tree) + junction orphans
   (FK cascade unenforced, prepare_delete scope) + rowid-keyed deletes no-oping in the frontend store.
3. **Ghost row**: boot-seeded raw-SQL layout rows are protected from deletion during the boot window by
   `projection.arm()` and hidden only by a query-level filter; combined with delete-propagation gaps they
   are the longest-lived ghosts. Cleanup is restart-deferred by design.
