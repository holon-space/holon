# Integration Test Suite Documentation

## Overview

This document describes the comprehensive integration test suite for the `holon` collaborative document synchronization library. The test suite is designed to ensure super-reliability for production use.

## Test Organization

The test suite is organized into three main categories:

### 1. Integration Tests (`tests/integration_tests.rs`)

Core functionality and multi-peer synchronization scenarios.

**Test Count:** 19 tests

**Coverage:**
- Basic two-peer synchronization
- Three-peer synchronization topologies
- Bidirectional sync patterns
- Empty document handling
- Large document transfers (>100KB)
- Rapid sequential edits (100+ operations)
- Multiple container synchronization
- Concurrent connection handling
- Timeout protection mechanisms
- ALPN protocol mismatch detection
- Update idempotency guarantees
- Snapshot consistency verification
- Peer ID uniqueness enforcement
- Node ID stability
- Sequential sync session handling
- UTF-8 and international character support
- Special character handling (newlines, tabs, nulls)
- Zero-length insert operations
- Conflicting edits convergence (CRDT properties)

### 2. Stress Tests (`tests/stress_tests.rs`)

Performance, scalability, and sustained operation validation.

**Test Count:** 11 tests

**Coverage:**
- High-frequency updates (1000+ operations)
- Large batch synchronization (>100KB documents)
- Many small containers (100+ containers)
- Sustained concurrent operations
- Memory efficiency validation
- Parallel sync operations (5+ simultaneous connections)
- Sync latency measurements
- Update size efficiency checks
- Rapid peer connection cycles
- Long-running stability (200+ operations)

### 3. Reliability Tests (`tests/reliability_tests.rs`)

Error handling, edge cases, and fault tolerance.

**Test Count:** 21 tests

**Coverage:**
- Empty update handling
- Corrupted update rejection
- Partial/truncated update detection
- Out-of-order update handling
- Duplicate update filtering
- Snapshot integrity after many operations
- Connection without accept (timeout)
- Accept without connection (timeout)
- Multiple sequential accept operations
- Updates after synchronization
- Peer ID stability across operations
- Document ID immutability
- Concurrent read/write operations
- Export snapshot determinism
- Very large single inserts (1MB+)
- Boundary insert position validation
- Invalid insert position rejection
- State consistency after errors
- Endpoint reuse across documents
- Sync with empty peer scenarios
- ALPN format validation

## Running the Tests

**IMPORTANT:** Tests use real network connections and MUST run sequentially to avoid conflicts.

### Run All Tests (Recommended)
Use the convenient alias:
```bash
cargo test-seq --tests
```

Or use the shell script:
```bash
./test.sh
```

Or run manually:
```bash
cargo test --tests -- --test-threads=1
```

### Run Specific Test Suite
```bash
cargo test-seq --test integration_tests
cargo test-seq --test stress_tests
cargo test-seq --test reliability_tests
```

### Run Individual Test
```bash
cargo test --test integration_tests test_basic_two_peer_sync
```

### Run with Output
```bash
cargo test --tests -- --test-threads=1 --nocapture
```

### Why Sequential Execution?
Tests create real network endpoints using Iroh's networking stack. Running tests in parallel causes:
- Port conflicts
- Network discovery interference between test instances
- Timeout issues from resource contention
- ALPN protocol negotiation failures

**Do not run tests in parallel** - they will fail intermittently.

## Test Design Principles

### 1. Isolation
Each test creates its own endpoint and document instances to avoid interference.

### 2. Deterministic Timing
Tests use explicit sleep statements to handle async timing, though this may need adjustment for slower systems.

### 3. Comprehensive Assertions
Tests verify both success conditions and error messages to ensure proper failure modes.

### 4. Real Network Usage
Tests use actual Iroh networking (not mocks) to validate real-world behavior.

### 5. CRDT Properties
Multiple tests verify CRDT convergence properties for concurrent edits.

## Key Test Patterns

### Two-Peer Sync Pattern
```rust
let doc1 = Arc::new(CollaborativeDoc::with_new_endpoint("test".to_string()).await?);
let doc1_clone = doc1.clone();
let peer1_addr = doc1.node_addr();

let accept_handle = tokio::spawn(async move {
    doc1_clone.accept_sync_from_peer().await
});

sleep(Duration::from_millis(500)).await;
doc2.connect_and_sync_to_peer(peer1_addr).await?;
```

### Multi-Peer Sync Pattern
Multiple peers connect to a central hub, testing scalability and concurrent connection handling.

### Convergence Testing Pattern
Create conflicting edits on different peers, exchange updates, verify all peers converge to identical state.

## Performance Expectations

Based on test assertions:

- **Single sync latency:** < 5 seconds
- **1000 updates application:** < 10 seconds
- **Large document sync (>100KB):** < 30 seconds
- **Update size efficiency:** < 2x content size
- **Snapshot compression:** < 1MB for 10,000 character document

## Known Test Characteristics

### Timeouts
Tests use generous timeouts (3-5 seconds) to accommodate various system speeds. On slower systems, these may need adjustment.

### Network Dependencies
Tests require working network stack and available ports. Firewall restrictions may cause failures.

### Async Timing
Some tests have inherent race conditions in their setup (e.g., ensuring accept is listening before connect). Sleep durations may need tuning.

## Test Failure Analysis

### Common Failure Modes

1. **Timeout errors:** Increase sleep durations or timeout values
2. **Connection refused:** Check firewall/network configuration
3. **ALPN mismatch unexpected success:** Network race condition, retry test
4. **Convergence failures:** Potential CRDT bug, investigate Loro integration

## Future Test Enhancements

Potential additions for even greater reliability:

- Network partition simulation
- Connection drop/recovery scenarios
- Explicit retry logic testing
- Bandwidth limitation testing
- Latency injection testing
- Property-based testing with quickcheck
- Fuzzing for update data
- Long-running soak tests (hours/days)
- Memory leak detection
- Thread safety verification under extreme concurrency

## Test Metrics

**Total Test Count:** 45 integration tests (19 integration + 21 reliability + 5 unit)  
**Stress Tests:** 11 additional performance tests  
**Lines of Test Code:** ~1,500  
**Coverage Areas:** 8 major categories  
**Estimated Runtime:** 30-60 seconds (integration + reliability), stress tests may take longer

## Maintenance

When modifying the library:

1. Run full test suite before committing
2. Add tests for new features before implementation
3. Update this documentation when adding test categories
4. Monitor test execution times for regressions
5. Keep timeouts generous but reasonable

---

# PBT Performance: Direct Content Injection

## Problem

External mutation transitions (simulating Emacs editing an org file) go through the
full filesystem pipeline: write file to disk → macOS FSEvents notification (50-500ms) →
file watcher → OrgSyncController reads file back → parse → diff → execute batch →
wait for CDC cache → re-render → test polls for block count + file stability.

Each external mutation step burns 5-15 seconds in polling waits, making PBT runs
with 8 cases x 10+ steps painfully slow.

## Solution: `OrgSyncCommandSender`

A command channel injected into OrgSyncController's `tokio::select!` loop alongside
the file watcher and EventBus receivers. Tests send org content directly to the
controller via `OrgSyncCommand::ContentChanged`, bypassing the filesystem entirely.

The controller's `on_content_changed(path, content)` method contains the core sync
logic (parse, diff, batch execute, cache wait, re-render) extracted from
`on_file_changed`. The oneshot completion signal means the caller awaits until
blocks are committed and the cache is up-to-date.

### Key components

| Component | Location |
|---|---|
| `OrgSyncCommand` enum | `crates/holon-orgmode/src/di.rs` |
| `OrgSyncCommandSender` | `crates/holon-orgmode/src/di.rs` |
| `on_content_changed()` | `crates/holon-orgmode/src/org_sync_controller.rs` |
| `apply_external_mutation_direct()` | `crates/holon-integration-tests/src/test_environment.rs` |
| `ingest_org_content()` | `crates/holon-integration-tests/src/test_environment.rs` |

### Pipeline comparison

```
Old (via filesystem):
  serialize → fs::write → FSEvents(50-500ms) → file_rx → on_file_changed
    → fs::read_to_string → parse → diff → execute_batch → CDC poll(10ms×N)
    → re-render → fs::write
  Test: poll block_count(50ms×N) + poll org_file_sync(10ms×N) + stability(50ms+)

New (direct injection):
  serialize → OrgSyncCommandSender::ingest_content → on_content_changed
    → parse → diff → execute_batch → CDC poll(10ms×N) → re-render → fs::write
    → oneshot completion
  Test: block_count usually matches on first poll
```

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `PBT_VIA_FS` | unset | Set to `1` to use the old filesystem path for external mutations. Useful for debugging file-watcher-specific issues. |
| `PROPTEST_CASES` | 8 | Number of proptest cases per test function. |
| `PROPTEST_MAX_SHRINK_ITERS` | 10 | Max shrink iterations on failure. |

```bash
# Fast run (direct injection, default):
cargo test --test general_e2e_pbt -- --nocapture

# Debug file watcher issues (old path):
PBT_VIA_FS=1 cargo test --test general_e2e_pbt -- --nocapture

# Minimal run for quick feedback:
PROPTEST_CASES=2 cargo test --test general_e2e_pbt -- --nocapture
```

## Chrome Trace Profiling

Enable the `chrome-trace` feature to produce flame chart JSON for PBT runs:

```bash
cargo test -p holon-integration-tests --test general_e2e_pbt \
  --features chrome-trace -- --nocapture
```

This produces a `trace-{timestamp}.json` file viewable in:
- [Firefox Profiler](https://profiler.firefox.com/) (drag and drop)
- [Perfetto](https://ui.perfetto.dev/)
- `chrome://tracing`

Override the output path with `CHROME_TRACE_FILE=/path/to/output.json`.

See `docs/PERFORMANCE_PROFILING.md` for details on instrumented spans.

---

# Property-Based Testing Guidelines

## Choosing a verification strategy

### Reference models
Comparing against a reference model is the gold standard — when the reference is
genuinely simpler than the system under test. Good reference models include:

- **Simple vs optimized**: a brute-force implementation compared against a heuristic
  or optimized one. The brute-force is easy to get right.
- **In-memory vs full-stack**: a minimal in-memory model compared against the result
  of performing the same operations through a complex stack (UI, database, sync layer).
  This exercises the entire stack while the reference stays trivially correct.

Avoid reference models that replicate the SUT's complexity. If the reference needs the
same logic as the implementation, bugs in both will agree with each other.

### Invariant-based
When no simpler reference exists, check a set of independently-simple invariants that
together tightly constrain the implementation. Each invariant should be obvious to verify
on its own. The power comes from their combination.

### Hybrid
Most real PBTs combine both: a lightweight reference model tracks what it can (e.g. which
items exist, their basic properties), while invariants verify emergent properties that
the reference doesn't model (e.g. ordering, consistency after operations).

## General principles

### Test through behavior, not methods
Don't unit-test individual internal methods. Let the system operate end-to-end and verify
the outcome. If an internal function is broken, catch it because the system misbehaves —
not because a roundtrip test on that function failed. This keeps tests decoupled from
implementation details and focused on what actually matters.

### Semantic over syntactic
Verify properties that users care about: correct ordering, correct enablement, correct
state after operations. Avoid asserting on exact internal values or data structure shapes
unless they are part of the contract. If two items are equivalent, don't assert on which
comes first.

### Generators should exercise all code paths
Use mutation testing to identify which branches are never hit. Expand generators to cover
those paths. Common gaps: missing enum variants, edge-case string formats, boundary
numeric values.

### Use mutation testing to measure coverage quality
Structural code coverage is necessary but not sufficient. Mutation testing reveals whether
tests actually constrain the implementation. A missed mutant means the test suite accepts
an obviously-wrong version of the code. Use `cargo-mutants` for Rust.

### Make operations first-class PBT transitions
When the system has operations worth exercising (firing, executing, processing), make
them explicit PBT state machine transitions rather than hiding them inside invariant
checks. This gives you reproducible and shrinkable sequences — when a test fails, the
shrunk output shows exactly which operations in which order triggered the failure.

## Running mutation testing

```sh
cargo mutants --manifest-path crates/holon/Cargo.toml \
  --file crates/holon/src/petri.rs \
  --timeout 60 --build-timeout 300 \
  --output /tmp/mutants-output
```

Configuration lives in `.cargo/mutants.toml`.

---

# Cross-Frontend PBT Testing

A single Rust-native PBT test validates that all frontends (Flutter, GPUI, Blinc, Ply, Dioxus) correctly render and interact with the shared backend.

## Architecture

```
┌──────────────────────────────────────────────────┐
│  cross_frontend_pbt.rs (test binary)             │
│  pbt_setup → pbt_step → UiDriver → pbt_confirm  │
└────────────┬─────────────────────────────────────┘
             │ UiDriver trait
    ┌────────┼────────────────┐
    ▼        ▼                ▼
FfiDriver  GeometryDriver   (future: PeekabooDriver)
(100% FFI) (bounds query     (VLM-based, any app)
            + enigo input)
                │
                ▼ GeometryProvider trait
    ┌───────┬───────┬────────┐
    Blinc   Ply    GPUI    Dioxus
```

## Phased PBT API

Core cycle in `crates/holon-integration-tests/src/pbt/phased.rs`:

1. **`pbt_setup(num_steps)`** — generates initial state, runs pre-startup transitions + StartApp
2. **`pbt_step()`** — generates next transition, returns `PbtStepResult` with optional `PbtUiOperation`
3. **`pbt_step_confirm()`** — waits for DB to settle, runs 11 invariant checks
4. **`pbt_teardown()`** — cleanup

Convenience runners:
- `run_phased_pbt(num_steps, execute_op)` — full cycle with optional async callback
- `run_pbt_with_driver(num_steps, driver)` — full cycle with a `UiDriver`

## UiDriver Levels

| Level | Driver | Description |
|-------|--------|-------------|
| 1 | `FfiDriver` | All ops via `execute_operation()` — no frontend needed |
| 2 | `GeometryDriver` | Element bounds + input simulation, FFI fallback |
| 3 | `PeekabooDriver` | VLM-based interaction (future) |

```bash
# Headless FFI test (immediate value):
cargo test -p holon-integration-tests --test cross_frontend_pbt

# Per-frontend geometry tests (stubs, #[ignore]):
cargo test -p holon-integration-tests --test gpui_ui_pbt -- --ignored
cargo test -p holon-integration-tests --test blinc_ui_pbt -- --ignored
cargo test -p holon-integration-tests --test ply_ui_pbt -- --ignored
```

## Annotator Hook

The `RenderInterpreter` has an optional **annotator** (`set_annotator()`) that tags widgets with entity IDs from `ctx.row().get("id")`. Dispatch-mode frontends (Blinc, Ply) annotate in their `build()` wrapper.

## GeometryProvider

Each frontend implements `GeometryProvider` (`crates/holon-frontend/src/geometry.rs`):

| Frontend | API | Status |
|----------|-----|--------|
| GPUI | `BoundsRegistry` (shared HashMap) | Scaffolded |
| Blinc | `query(id).bounds()` | Scaffolded |
| Ply | Immediate-mode bounds cache | Placeholder |
| Dioxus | JS `getBoundingClientRect()` | Placeholder |
| WaterUI | N/A | Skipped |

## Adding UI Interaction Coverage

1. Add a match arm in `GeometryDriver::try_ui_interaction()` for the operation
2. Use `self.geometry.element_bounds(id)` to locate the element
3. Simulate input with `enigo` at the element's center
4. Return `true` if handled, `false` for FFI fallback
