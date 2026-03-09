# LiveData migration — remaining work

Status: invariant-side migration shipped; transition-side reads and base-table
CDC still on the table. This document lays out the two follow-ups, what's
already known, and the smallest experiments that would unblock each.

## Context (what landed)

- `MatviewManager` view-name cache + atomic counters (`cache_hits`,
  `exists_calls`, `ddl_creates`); accessor on `BackendEngine`.
- `BackendEngine::watch_view(sql)` exposed for tests.
- `parse_block_row()` helper in `crates/holon-integration-tests/src/pbt/sut.rs`
  is the single source of truth for SQL-row → `Block` conversion.
- `E2ESut` carries three lazy-init `LiveData` accessors:
  - `live_blocks()` — `LiveData<Block>` over `SELECT … FROM block`
  - `live_block_tags()` — `LiveData<BlockTag>` over `block_tags` (joined into
    `Block.tags` in Rust at the call site since Turso IVM rejects correlated
    subqueries in matview defs)
  - `live_focus_roots()` — `LiveData<FocusRoot>` over the `focus_roots` matview
- inv1, inv7, inv8 now read these snapshots instead of issuing SQL.
  `wait_for_consumers` is the synchronization barrier — validated as
  delay-free over hundreds of invariant invocations across all 3 PBT variants.
- Counter Drop print on `E2ESut` gated behind `PBT_MATVIEW_METRICS=1`.

Wall-time delta from this: within the ±10 s/case noise floor at
`PROPTEST_CASES=1`. Architectural delta is the real win — uniform pattern,
typed structs, no SQL coupling for invariant reads.

---

## Follow-up 5 — Where do all the SQL queries come from?

### Premise

The original profile attributed **1739 queries × 9 ms = 15.78 s** to one PBT
case. Invariants account for roughly 5 queries × ~30 checks ≈ **150** of those.
The other **~1600** come from somewhere else, and the user has confirmed seeing
the same volume in the logs. **Something is not right** — the system should not
be issuing that many queries per case.

This is the lever, not "rewrite transition apply paths to use LiveData." Until
we know where the queries originate we can't responsibly cache or batch them.

### Investigation plan

1. **Add a per-target query counter to `BackendEngine`.** `MatviewManager`
   already has atomic counters; mirror them on the query path:
   - `BackendEngine::execute_query` increments a global `Atomic<u64>`.
   - Group by callsite via a small `tracing::Span` field (e.g.
     `query.origin = "transition_apply" | "org_sync" | "invariant" | "fdw_prime"`).
     Wrap each call site that uses `.execute_query` / `.query_sql` /
     `subscribe_sql` to set `query.origin` once at the top of the span tree.
   - Expose totals on `E2ESut` Drop so we get per-case numbers.

2. **Run one PBT case with `RUST_LOG="warn,holon::api=debug"` and tee'd
   output**, then `grep -c` per origin. The 1600 missing queries should fall
   into 1–3 buckets.

3. **Likely suspects to check first** (rank by hypothesis weight):
   1. **CDC echo loops in OrgSyncController** — every block CDC event causes
      an org file rewrite, which the file watcher picks up, which queries to
      figure out what changed. If echo suppression is leaky, you get N²
      blowup.
   2. **Per-block lookups during transition apply** — e.g. the
      `apply_transition` path in PBT may issue one query per block touched
      to look up parent/sibling state.
   3. **`prime_fdw_caches`** — it runs unconditionally inside `ensure_view`
      and may issue FDW reads even when the result is already cached.
      `fdw_backed_tables.is_empty()` early-returns it; check whether the PBT
      ever registers FDW tables (MCP integrations etc.).
   4. **`reconcile_named_view`** at `crates/holon/src/sync/matview_manager.rs:44` —
      compares against `sqlite_master`; if any caller invokes it on a hot path
      (per CDC event, per org write), it adds 1 query each.
   5. **The reactive engine** subscribing & re-querying on each structural
      re-render. Each `watch_ui` triggers `query_and_watch`; if structural
      events fire often, this multiplies.

4. **Cross-check against the matview cache counters.** A single case shows
   ~30 ensure_view calls and ~150–300 invariant queries in our experiments.
   The 1739-figure baseline must be either (a) a longer/instrumented run or
   (b) a code path that bypasses the matview manager. (b) is the more
   interesting case — find it.

### Once located

- If it's a real bug (echo loop, missing dedup): fix it. The wall-time win
  should dwarf anything LiveData can offer.
- If it's a legitimate pattern (e.g. transition apply genuinely needs to look
  up neighbours): consider a `LiveData<Block>` reuse pattern for transition
  paths. The reading pattern is the same as on the test side; only the
  watermark scoping differs.

### Files / search anchors

- `crates/holon/src/api/backend_engine.rs` (`execute_query`, `query_and_watch`,
  `subscribe_sql`)
- `crates/holon/src/api/holon_service.rs` (`execute_query`, `execute_sql`)
- `crates/holon/src/sync/matview_manager.rs` (`ensure_view`, `prime_fdw_caches`)
- `crates/holon-orgmode/src/file_watcher.rs` and the OrgSyncController on the
  echo-loop suspicion
- `crates/holon-integration-tests/src/pbt/sut.rs` (`apply_transition_async`)

### Effort estimate

Investigation: 2–4 hours. Fix scope: depends on what it turns out to be —
could be a one-line dedup, could be a structural rework. Don't budget without
data.

---

## Follow-up 5 — Status: closed (May 2026 investigation)

The instrumentation hypothesis was completed. `SpanCollector::queries_by_origin()`
walks each `query` span's ancestor chain and tallies counts + durations per
chain. `E2ESut` accumulates per-transition snapshots and prints the merged
breakdown on `Drop` when `PBT_MATVIEW_METRICS=1`.

What we found:

- **Per-case query count is ~125–180 reads, not 1739.** The original baseline
  was stale — almost certainly captured before the matview-name cache and the
  invariant-side `LiveData` migration landed. Both `_sql_only` and `Full`
  variants land in the same range.
- **OrgSync is the biggest remaining bucket in `Full`.** `org.poll_external_changes ▸
  org.on_file_changed` (21× / 311 ms) plus `org.initial_scan.ingest ▸
  org.on_file_changed` (14× / 156 ms) account for ~33 % of case time. The
  handoff's #1 hypothesis (CDC echo loop in OrgSyncController) is partially
  visible but is not catastrophic — 2 ms/query, not the runaway loop the
  baseline implied.
- **`<no-parent>` queries (~17×, 471 ms in the heaviest case)** come from
  spawned tasks that didn't propagate a parent span. Real candidates if you
  want to push further: instrument the actor spawn points so future runs can
  attribute these to a subsystem.
- The other suspects (`prime_fdw_caches`, `reconcile_named_view`, reactive
  re-querying) do **not** appear as significant buckets.

Conclusion: the "missing 1600 queries" mystery is gone — prior work already
solved it. The instrumentation is durable test infra and stays even though
the original investigation closed.

---

## Follow-up 6 — Status: falsified (May 2026 experiment)

The premise was: `subscribe_cdc("<table>")` could replace
`MatviewManager::watch("SELECT … FROM <table>")` and skip IVM bookkeeping
because base-table writes might emit CDC events keyed on the table name.

`crates/holon/src/storage/cdc_base_vs_matview_repro.rs` proves the opposite:

- Direct `INSERT`/`UPDATE`/`DELETE` against a base table produces **zero**
  CDC batches with `relation_name == "<table>"` on the broadcast channel.
- Only matviews that `SELECT` from the table emit CDC batches, with
  `relation_name == "<matview_name>"`.
- Multi-statement transactions behave the same way.

Implication: the matview is the *only* mechanism by which Turso surfaces row
changes. There is no overhead-free path. The 3 PBT `LiveData` instances must
keep going through `MatviewManager::watch`.

The reproducer is kept as durable characterization so this doesn't get
re-investigated next time.

---

## Follow-up 6 — Original premise (kept for context)

### Premise

Today every `LiveData<T>` we set up goes through `BackendEngine::watch_view`,
which calls `MatviewManager::ensure_view` to build a matview that mirrors a
`SELECT … FROM table`. The matview is created, the CDC stream is subscribed,
LiveData populates from initial query + stream.

For base tables this is overkill: the table itself produces CDC events (the
broadcast channel routes by `relation_name`, see `MatviewManager::spawn_demux`
at `crates/holon/src/sync/matview_manager.rs:159`). In principle:

```text
mgr.subscribe_cdc("block")  // already routes when relation_name = "block"
```

…would work without ever building a matview. Saves IVM bookkeeping per CDC
event and avoids the `CREATE MATERIALIZED VIEW` round trip on first watch.

### What we don't know

The user's note: **"I'm not sure if CDC with callbacks for tables is
implemented exactly the way it is for MatViews."**

Unverified assumptions:

1. Whether base-table CDC events carry the same `RowChange` shape (Created /
   Updated / Deleted with full row data) as matview CDC events. If matview
   CDC includes only the projected columns from the SELECT and base-table CDC
   includes raw column dumps, the `parse_fn` for `LiveData` would need
   different handling for each.
2. Whether the demux routes base-table events identically. Looking at
   `MatviewManager::spawn_demux`, the demux routes by `batch.metadata.relation_name`
   matched against subscribed view names — base-table names would need to be
   registered the same way. Unclear if that's already supported or if the demux
   filters out non-`watch_view_*` names.
3. Whether ordering / batching guarantees match. Matview CDC is delivered after
   IVM has folded in the change; base-table CDC may fire pre-IVM, which means
   chained matviews (e.g. `focus_roots` depends on `block`) might see different
   timing relative to base-table consumers.
4. Whether deletes propagate. Some CDC implementations only fire on writes;
   some on triggers; some on transactions. The matview path goes through IVM,
   which is well-understood. Base-table path may have different completeness.

### Experiment plan

A small integration test, modelled on existing reproducers
(`crates/holon/src/storage/turso_ivm_*_repro.rs`):

1. Open a fresh DB.
2. Subscribe to base-table CDC for `block` via `mgr.subscribe_cdc("block")`.
3. In parallel, subscribe via `mgr.watch("SELECT … FROM block")` (matview path).
4. Run a sequence: INSERT, UPDATE, DELETE, transactional batch insert, two
   concurrent writers.
5. After each, assert both streams produce the same `RowChange` events for the
   same rows, in the same order, with the same column data.

If the streams diverge:
- Document the divergence in a new memory entry.
- Either (a) shim the difference behind a `subscribe_table` API on
  `MatviewManager`, or (b) keep going through matviews because the cost is
  acceptable.

If they match:
- Add `BackendEngine::watch_table(name)` that returns a `WatchResult` without
  the matview overhead.
- Migrate the three PBT `LiveData` instances to it.
- Re-run the matview cache metrics — `ddl_creates` for the three test matviews
  should drop to zero.

### Files / search anchors

- `crates/holon/src/storage/turso.rs` — `cdc_broadcast`, `RowChange`,
  `RowChangeStream`
- `crates/holon/src/sync/matview_manager.rs:159` — `spawn_demux` (the routing
  layer)
- `crates/holon/src/sync/matview_manager.rs:430` — `subscribe_cdc` (already
  takes any view/table name, so demux registration may already be uniform)
- Existing reproducers under `crates/holon/src/storage/turso_*_repro.rs` for
  the testing pattern

### Effort estimate

Experiment: 1–2 hours (single-file integration test).
Migration if streams match: 30 min.
Migration if streams diverge: depends on divergence; most likely 0 (keep
matviews) or up to a half-day (build a shim).

---

## Why these are still on the list

The materialized invariant migration captured ~5–10% of the theoretical
ceiling from the original profile. The remaining ~90% is split between:

- **Follow-up 5** — fixing whatever is producing the 1600 mystery queries
  (likely the bigger lever; could be structural).
- **Follow-up 6** — saving the matview overhead per LiveData consumer (smaller
  but architecturally cleaner; worth it if streams turn out to be equivalent).

Both are bounded investigations with clear stopping conditions. Either is a
good standalone task for a future session.
