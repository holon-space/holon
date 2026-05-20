# PBT Root-Cause Analysis Plan

**Date**: 2026-04-28  
**Context**: `crates/holon-integration-tests/tests/general_e2e_pbt.rs`  
**Scope**: 4 issues found during test run, plus execution-speed diagnosis

---

## Issue Map

| #   | Issue                                          | Severity   | Category                            |
| --- | ---------------------------------------------- | ---------- | ----------------------------------- |
| 1   | inv10i IVM Matview Inconsistency               | **High**   | Correctness — stale/ghost data      |
| 2   | N+1 `SELECT MAX(created_at)` on `events` table | **Medium** | Performance — 50-83× per transition |
| 3   | Consumer watermark timeouts (loro, org, cache) | **Medium** | Infrastructure — 500ms too low?     |
| 4   | RSS memory growth 1.2→1.7GB per case           | **Medium** | Resource — possible OTel span leak  |

---

## Phase 1: Isolate Each Issue (Sequential, ~30 min each)

### 1A — inv10i Matview Inconsistency

**What we know**: The matview accumulates state from previous PBT cases. "Extra in matview" blocks (`block:3226058d-...`, `block:65e86eb2-...`, `block:b79af698-...`) appear with IDs that are domains of previous test runs' randomized UUIDs, suggesting the test environment doesn't fully reset the matview between cases.

**Hypothesized root cause**: The PBT `ReferenceState` resets per case, but the SUT's Turso SQLite database (and its materialized views) persists across cases within the same process. The matview `AppState.data_rows` retains entries from prior cases that weren't cleaned up by `DROP`/`CREATE MATERIALIZED VIEW`.

**Investigation steps**:

1. **Add targeted tracing to Reset path**
   - Instrument `E2ESut::init_test()` — verify `self.ctx` is fully torn down
   - Check `AppState.reset()` — does it `DROP` matviews?
   - Check `DbHandle.reset()` / `DbHandleProvider` lifecycle
   - Add `SELECT COUNT(*) FROM watch_view_*` dump at start of each case

2. **Reproduce with 2-case isolation**

   ```bash
   PROPTEST_CASES=2 cargo test -p holon-integration-tests \
     --test general_e2e_pbt -- general_e2e_pbt --nocapture 2>&1 | \
     grep -E "inv10i|\[init_test\]|ref_state has"
   ```

   - Case 1: Should NOT show inv10i (fresh DB)
   - Case 2: If inv10i shows "extra" blocks from Case 1 → confirmed persistence leak

3. **Check matview lifecycle**
   - `rg -n "DROP.*watch_view|CREATE MATERIALIZED VIEW.*watch_view|reset.*matview" crates/holon/src/`
   - `rg -n "destroy|teardown|cleanup|reset" crates/holon-integration-tests/src/pbt/sut.rs`
   - Focus: `E2EContext::reset_for_new_test()` / equivalent

4. **Fix candidates**
   - Option A: `DROP MATERIALIZED VIEW IF EXISTS watch_view_*` at `init_test` start
   - Option B: Create a fresh in-memory SQLite DB per case
   - Option C: Add `AppState.reset()` call between cases

**Key files**:

- `crates/holon-integration-tests/src/pbt/sut.rs` — inv10i check at line ~4067, E2EContext lifecycle
- `crates/holon/src/storage/turso_matview_test.rs` — matview management
- `crates/holon/src/storage/turso.rs` — matview CREATE/DROP

---

### 1B — N+1: `SELECT MAX(created_at) AS ts FROM events WHERE processed_by_loro = 1`

**What we know**: This query fires 50-83× per transition. It's called from `consumer_position("loro")` in `turso_event_bus.rs:566`, which is the heartbeat/wait loop that polls for watermark convergence in `wait_for_consumers()`.

**Hypothesized root cause**: The `wait_for_consumers` polling loop calls `consumer_position()` for each consumer in a tight retry loop with no backoff. Every transition calls `wait_for_consumers` at least once (in `apply_transition_async`), and some transitions (NavigateFocus, StartApp, BulkExternalAdd) hit multiple drain cycles that each poll the watermark.

**Investigation steps**:

1. **Chrome trace the polling pattern**

   ```bash
   CHROME_TRACE_FILE=trace-events-nplus1.json \
   CHROME_TRACE_FILTER="holon=info,holon::sync=trace" \
   PROPTEST_CASES=1 cargo test -p holon-integration-tests \
     --features chrome-trace \
     --test general_e2e_pbt -- general_e2e_pbt --nocapture
   ```

   - Open `trace-events-nplus1.json` in `https://ui.perfetto.dev/`
   - Search for `events.mark_processed` — visualize call tree
   - Expected: thousands of dots marking each `SELECT MAX(created_at)` call
   - Look for tight clusters → polling without backoff

2. **Add exponential backoff benchmark**
   - Instrument `wait_for_consumers` in `turso_event_bus.rs`:
     ```rust
     let mut poll_count: usize = 0;
     while watermark != target {
         poll_count += 1;
         if poll_count % 100 == 0 {
             tracing::warn!("wait_for_consumers: {poll_count} polls so far...");
         }
     }
     ```
   - Run single transition and dump poll counts
   - If poll count >100 → confirm tight loop

3. **Check the retry interval**
   - `rg -n "wait_for_consumers|consumer_position|watermark" crates/holon-integration-tests/src/pbt/sut.rs`
   - `rg -n "sleep|interval|delay" crates/holon/src/sync/turso_event_bus.rs`
   - If no `sleep`/`tokio::time::sleep` in the polling loop → no backoff

4. **Fix candidates**
   - Option A: Add `tokio::time::sleep(Duration::from_millis(1))` in poll loop
   - Option B: Use `notify`/`watch`-based approach instead of polling
   - Option C: Cache last known position, only re-query after CDC events drain
   - Option D: Bump test timeout to 2s (acceptable for PBT, avoids watermark races)

**Key files**:

- `crates/holon/src/sync/turso_event_bus.rs:536-568` — `watermark()` and `consumer_position()`
- `crates/holon-integration-tests/src/pbt/sut.rs` — `wait_for_consumers` calls
- `crates/holon-integration-tests/src/test_tracing.rs` — `duplicate_sql` detection (already in place)

---

### 1C — Consumer Watermark Timeouts

**What we know**: `[wait_for_consumers] timeout: consumers ["loro", "org", "cache"] did not reach watermark within 500ms` appears on almost every transition. Often followed by `[wait_for_loro_quiescence] timeout after 500ms`.

**Hypothesized root cause**: Two possibilities:

1. **Timeout too low** — 500ms is not enough for CDC events to propagate through `loro→org→cache` pipeline
2. **Consumer genuinely stuck** — the `loro` consumer isn't processing events, causing `processed_by_loro` to never advance

**Investigation steps**:

1. **Profile consumer processing times**
   - Add span tracing to consumer processing:
     ```rust
     #[tracing::instrument(skip(self), fields(consumer = %consumer))]
     fn process_event(&self, event: &Event) { ... }
     ```
   - Run with `CHROME_TRACE_FILTER="holon::sync=trace"`
   - Check: are consumers processing events at all? How long per event?

2. **Distinguish stuck vs slow**
   - Add counter: `events_processed_by_loro_since_last_poll`
   - If counter == 0 → consumer is stuck (no work happening)
   - If counter > 0 but watermark hasn't advanced → processing is just slow

3. **Check loro consumer startup**
   - Loro consumer has a multi-stage initialization (`LoroModule STAGE 1-3, SEED-STAGE 1-3`)
   - If the consumer is still initializing when the test hits `wait_for_consumers`, it'll time out
   - Search for: `[LoroModule]` / `[LoroSyncController]` in logs

4. **Fix candidates**
   - Option A: Increase timeout to 2s (generous but acceptable for PBT)
   - Option B: Add upfront await for consumer ready signal before transitions
   - Option C: Skip watermark check if Loro is disabled (variant check)

**Key files**:

- `crates/holon/src/sync/loro_sync_controller.rs` — consumer processing
- `crates/holon/src/sync/event_bus.rs` — event dispatch
- `crates/holon/src/sync/turso_event_bus.rs` — watermark infrastructure

---

### 1D — RSS Memory Growth (1.2GB → 1.7GB)

**What we know**: Per-transition RSS deltas of +14-18MB, cumulative grows to +169MB within a single PBT case. The `startapp` transition produces 226K OTel spans (stored in `InMemorySpanExporter` until `reset()`).

**Hypothesized root cause**: `InMemorySpanExporter` in `SpanCollector` retains all spans until the next transition's `reset()`. With 50-83 `events` queries per transition × N transitions, the span vector grows significantly. The reset() happens at the start of each transition, but not all memory is reclaimed immediately (Rust's allocator may hold pages).

**Investigation steps**:

1. **dhat heap profile**

   ```bash
   cargo test -p holon-integration-tests \
     --features heap-profile \
     --test general_e2e_pbt -- general_e2e_pbt --nocapture &
   # Wait for memory to grow, then:
   kill -SIGINT <pid>  # dhat flushes dhat-heap.json
   ```

   - Open `dhat-heap.json` at https://nnethercote.github.io/dh_view/dh_view.html
   - Sort by "total bytes" — look for:
     - `InMemorySpanExporter` retaining spans
     - `SpanData` allocations
     - Any unexpected retain cycles

2. **Span count audit**
   - Add logging: `tracing::info!("SpanCollector has {} spans before reset", self.exporter.len());`
   - Count spans per transition type
   - Check: does `reset()` actually clear spans? (InMemorySpanExporter wraps `Arc<Mutex<Vec>>`)

3. **Check for non-span memory growth**
   - `ps` output shows RSS but not heap/fragmentation breakdown
   - Use `memory_stats::memory_stats()` to get `physical_mem` and `virtual_mem`
   - Run with `MALLOC_LOG=1` on Linux, or Instruments on macOS:

4. **Fix candidates**
   - Option A: Downsample spans — only keep spans with duration > 1ms
   - Option B: Call `SpanCollector::reset()` after `check_invariants`, not just before `apply`
   - Option C: Use a bounded ring-buffer instead of unbounded Vec
   - Option D: Pre-allocate expected span count to reduce allocator churn

**Key files**:

- `crates/holon-integration-tests/src/test_tracing.rs` — `SpanCollector`, `InMemorySpanExporter`
- `crates/holon-frontend/src/memory_monitor.rs` — `MemoryMonitor`, `dhat`, `heap_profile`
- `crates/holon-integration-tests/src/pbt/transition_budgets.rs` — `MemoryMetrics`, `diagnose_memory()`

---

## Phase 2: Execution Speed Diagnosis (~1 hr)

**Observation**: Each PBT case takes 30-90s+. With 8 cases × 3 variants = 24 cases, the full test takes 12-36 minutes.

### 2A — Chrome Trace for Wall-Clock Attribution

```bash
CHROME_TRACE_FILE=trace-pbt-full.json \
CHROME_TRACE_FILTER="holon=info,holon::api=debug,holon_frontend=debug,holon_integration_tests=info" \
PROPTEST_CASES=1 cargo test -p holon-integration-tests \
  --features chrome-trace \
  --test general_e2e_pbt -- general_e2e_pbt --nocapture
```

Open `trace-pbt-full.json` in Perfetto and check:

- **`pbt.apply_transition`** span durations → which transitions dominate?
- **`pbt.check_invariants`** span durations → invariant checking overhead
- **`pbt.drain_cdc_events`** → drain timeouts (1s/200ms)
- **`events.mark_processed`** → N+1 waterfall
- **`pbt.wait_for_org_file_sync`** → 5s timeout called out in comments

### 2B — Flamegraph Generation

```bash
HOLON_PERF_FLAMEGRAPH=/tmp/pbt-flamegraphs \
PROPTEST_CASES=1 cargo test -p holon-integration-tests \
  --test general_e2e_pbt -- general_e2e_pbt --nocapture

# Generate flamegraph SVGs
ls /tmp/pbt-flamegraphs/*.folded | while read f; do
  cat "$f" | inferno-flamegraph > "${f%.folded}.svg"
done
```

### 2C — Speed Hotspots (speculative from existing metrics)

| Suspect                                              | Evidence                                     | Tool                        |
| ---------------------------------------------------- | -------------------------------------------- | --------------------------- |
| `wait_for_org_file_sync` hitting 5s timeout          | `inv13` logs show 5002ms                     | Chrome trace                |
| `drain_cdc_events` with 1s timeout per call          | Multiple `drain_cdc` calls per transition    | Chrome trace span durations |
| `pbt.check_invariants` SQL reads (SELECT all blocks) | Every transition re-reads entire block table | Flamegraph                  |
| N+1 `SELECT MAX(created_at)` 50-83×                  | Already identified                           | Span counts                 |

---

## Phase 3: Fix Plan (Ordered by impact)

### Priority 1: inv10i Matview Inconsistency (correctness bug)

1. Confirm: persistence leak between PBT cases via `2-case isolation test`
2. Fix: `AppState.reset()` or `DROP MATERIALIZED VIEW` at `init_test` boundary
3. Verify: Run 2 cases, check no extra blocks in inv10i

### Priority 2: N+1 Watermark Polling (performance)

1. Confirm: tight loop via Chrome trace + poll-count instrumentation
2. Fix: Add `tokio::time::sleep(Duration::from_millis(1))` in poll loop OR switch to notify-based
3. Verify: SQL read budget violations drop from 50-83x to <5x for watermark query

### Priority 3: Watermark Timeouts

1. Investigate: is consumer stuck or just slow?
2. Fix: Increase timeout to 2s OR skip when applicable
3. Verify: `wait_for_consumers timeout` count drops

### Priority 4: Memory Growth

1. Profile: dhat heap profile
2. Fix: Bounded span ring-buffer or earlier reset()
3. Verify: RSS delta per transition drops below 10MB limit

### Priority 5: Test Execution Speed

1. Profile: Chrome trace + flamegraph
2. Fix: Optimize top 3 slowest operations
3. Stretch: Get single case under 30s

---

## Tooling Reference

| Tool           | Command/Feature                                                            | Output                                  |
| -------------- | -------------------------------------------------------------------------- | --------------------------------------- |
| Chrome Trace   | `--features chrome-trace` + `CHROME_TRACE_FILE=`                           | JSON → Perfetto/Firefox Profiler        |
| Flamegraph     | `HOLON_PERF_FLAMEGRAPH=/dir`                                               | .folded files → inferno-flamegraph      |
| Heap Profile   | `--features heap-profile` + SIGINT                                         | `dhat-heap.json` → dhat viewer          |
| Memory Monitor | Built-in `MemoryMonitor`                                                   | Logs RSS every 30s                      |
| Span Collector | `SpanCollector::global()`                                                  | `sql_read_count`, `duplicate_sql`, etc. |
| Budget Checker | `check_budget()` in `transition_budgets.rs`                                | Per-transition SQL/memory violations    |
| env vars       | `PBT_MEMORY_MULTIPLIER`, `PROPTEST_MAX_SHRINK_ITERS`, `HOLON_RSS_ABORT_MB` | Tuning knobs                            |
