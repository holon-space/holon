# Handoff: Zero-Tolerance SQL Query Budgets for PBT

## Goal

Eliminate the `tolerance` field from `ExpectedSql` in `transition_budgets.rs`. Every SQL query a transition performs must be accounted for by the formula — no padding, no fuzz. If the formula says 14 reads and the system does 15, that's a bug (either in the formula or the code).

## What exists now

**Files:**
- `crates/holon-integration-tests/src/test_tracing.rs` — `SpanCollector` wrapping OTel `InMemorySpanExporter`, captures all `#[tracing::instrument]` spans from `turso.rs`
- `crates/holon-integration-tests/src/pbt/transition_budgets.rs` — `expected_sql(transition, ref_state)` computes expected reads/writes/ddl from `ReferenceState`
- Invariant 13 in `sut.rs` calls `check_budget()` after each transition, logs `actual/expected`

**Current accuracy** (from `[inv13]` output, format is `actual/expected`):
- `SwitchView`: 5/5 — exact
- `RemoveWatch`: 5/5 — exact
- `SetupWatch`: 9/9, ddl 1/1 — exact
- `NavigateFocus`: 14/14 — exact
- `SimulateRestart`: 7-9/9 — off by 2 (actual sometimes lower)
- `ApplyMutation::Update` (0 watches): 15-19/16-18 — off by 1-3
- `CreateDocument`: 12-18/13 — off by up to 5
- `ApplyMutation::Create` (0 watches, 28 blocks): **59/11** — off by 48

## The problem: internal watches

`ref_state.active_watches` only counts **user-created query watches** (from `SetupWatch` transition). But the system creates **internal watches** during startup that also trigger matview re-checks on every CDC event:

1. **Region watches** — `setup_region_watch()` in `sut.rs` creates 3 watches (left_sidebar, main, right_sidebar) via `query_and_watch()` on `focus_roots` matview
2. **All-blocks watch** — `setup_all_blocks_watch()` creates 1 watch on `SELECT id FROM block WHERE name IS NULL`
3. **Root layout watches** — the reactive engine's `watch_ui()` creates matviews for the root layout block
4. **Profile watcher** — may create a matview for `block WHERE properties IS NOT NULL`
5. **Navigation matviews** — `current_focus`, `focus_roots`, `current_editor_focus` are materialized views that get invalidated on block changes

Each internal watch adds `READS_PER_WATCH` (2 reads: sqlite_master check + turso internal check) per CDC event during a mutation. The 59-read Create case has ~8-12 internal matviews being checked, plus the reactive engine re-rendering multiple times as CDC events cascade.

## What needs to happen

### Step 1: Count internal watches

Add an `internal_watch_count` field to `E2ESut` (or `TestEnvironment`). After `start_app()`, count how many `query_and_watch()` calls happened by:
- Counting `region_streams` entries (3 for left/main/right)
- Counting `all_blocks_stream` (1 if present)
- Counting reactive engine root layout matviews

Or alternatively: add a counter to `TursoBackend` or `BackendEngine` that increments on every `query_and_watch()` call, then read it after startup.

### Step 2: Model CDC cascade multiplier

When a mutation happens, the CDC chain works like this:
1. Mutation → INSERT event → UPDATE events processed
2. CacheEventSubscriber picks up event → upserts into QueryableCache
3. Each matview that depends on the affected table gets invalidated → re-queried
4. Each re-query may trigger another reactive re-render cycle (base-5 reads)

The number of re-render cycles depends on:
- How many matviews depend on `block` table (all of them, basically)
- Whether the org sync re-writes the file (triggers file watcher → re-parse → more CDC events)
- The cascade depth: 1st mutation → org sync → 2nd CDC batch → more re-renders

**Key insight from the data**: with 28 blocks and 0 user watches, Create does 59 reads. The formula gives 11 (base 5 + journal 3 + doc lookup + name IS NULL + block existence). The ~48 extra reads are from:
- Internal watches checking matview existence: ~12 internal matviews × 2 reads = 24
- Multiple re-render cycles from CDC cascade: ~3 cycles × base 5 = 15
- Org sync re-read: properties + name IS NULL + doc lookup again = 9

So the formula should be roughly: `base_formula + internal_watches * READS_PER_WATCH + cdc_cycles * BASE_REACTIVE_READS + org_sync_overhead`

### Step 3: Determine CDC cycle count

The number of CDC cycles per mutation is deterministic but depends on:
- Whether the mutated block is in a document with org sync (always yes in PBT)
- Whether the render source block is affected (triggers structural re-render)
- How many focus_roots regions are dirtied

This could be modeled as a constant per mutation type:
- Create/Delete: 2-3 CDC cycles (mutation event + org sync event + matview invalidation)
- Update: 1-2 cycles (mutation event + optional org sync if content changed)
- NavigateFocus: 1 cycle (cursor update)

### Step 4: Remove tolerance field

Once the formula accounts for internal watches + CDC cycles + org sync overhead, the tolerance can be removed. The formula becomes:
```rust
reads = base_formula(mutation_kind)
      + internal_watches * READS_PER_WATCH
      + cdc_cycles * BASE_REACTIVE_READS
      + org_sync_reads(has_org_sync)
```

## How to validate

Run with `HOLON_PERF_DETAIL=1` to see exact SQL breakdown per transition:
```bash
HOLON_PERF_DETAIL=1 PROPTEST_CASES=5 cargo test -p holon-integration-tests \
  --test general_e2e_pbt general_e2e_pbt_sql_only -- --nocapture 2>&1 | tee /tmp/pbt.log
```

Look for `[inv13 DETAIL]` lines — they show every unique SQL query grouped by type (reads/writes/ddl) with occurrence counts. The `actual/expected` format in `[inv13]` lines shows how close the formula is.

## Key files to read

- `crates/holon-integration-tests/src/pbt/transition_budgets.rs` — the formulas (this is what you're changing)
- `crates/holon-integration-tests/src/test_tracing.rs` — SpanCollector, TransitionMetrics, SqlBreakdown
- `crates/holon-integration-tests/src/pbt/sut.rs` — invariant 13 (search for `inv13`), `setup_region_watch`, `setup_all_blocks_watch`
- `crates/holon/src/storage/turso.rs` — `#[tracing::instrument]` on query/execute/execute_ddl (the span names)
- `crates/holon/src/sync/turso_event_bus.rs` — CDC event processing (drives re-render cycles)
- `crates/holon/src/sync/matview_manager.rs` — matview invalidation logic
