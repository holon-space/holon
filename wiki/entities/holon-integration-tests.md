---
title: holon-integration-tests crate (PBT)
type: entity
tags: [crate, testing, pbt, property-based-testing, e2e]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon-integration-tests/src/lib.rs
  - crates/holon-integration-tests/src/pbt/mod.rs
  - crates/holon-integration-tests/src/pbt/state_machine.rs
  - crates/holon-integration-tests/src/pbt/transitions.rs
  - crates/holon-integration-tests/src/pbt/reference_state.rs
  - crates/holon-integration-tests/src/pbt/sut.rs
  - crates/holon-integration-tests/src/test_environment.rs
  - crates/holon-integration-tests/src/display_assertions.rs
---

# holon-integration-tests crate

Cross-crate **Property-Based Testing (PBT)** infrastructure and E2E integration tests. The primary quality gate for the entire system.

## Golden Rule

**Never add new PBT tests. Only use `general_e2e_pbt.rs`.** If a bug can't be reproduced by the PBT, make the PBT and production more similar (same code path, same data flow) so it can.

## Test Entrypoint

`crates/holon-integration-tests/tests/general_e2e_pbt.rs` — the main E2E PBT. Runs proptest state machine with phased execution.

## PBT Architecture

```
crates/holon-integration-tests/src/pbt/
├── mod.rs              # Public API: re-exports
├── state_machine.rs    # Proptest state machine implementation
├── transitions.rs      # E2ETransition enum (all possible operations)
├── reference_state.rs  # ReferenceState — in-memory model of expected system state
├── generators.rs       # proptest generators for blocks, queries, mutations
├── sut.rs              # E2ESut — System Under Test wrapper
├── phased.rs           # pbt_setup, pbt_step, pbt_teardown, run_phased_pbt
├── query.rs            # TestQuery, WatchSpec
├── types.rs            # Shared test types
├── loro_sut.rs         # Loro-specific SUT operations
└── loro_sync/          # Loro P2P sync test helpers
```

### ReferenceState

`crates/holon-integration-tests/src/pbt/reference_state.rs` — the **ground truth model** that the real system is compared against. Pure Rust, no async, no database.

Tracks:
- All blocks (as `HashMap<EntityUri, Block>`)
- Document tree structure
- Navigation state (current focus, history)
- Active watch specs (`WatchSpec` per region)
- Render expressions per render source block

Key method: `expected_focus_root_ids()` — what blocks should be visible given current navigation state. Used by PBT invariants.

`render_expressions: HashMap<String, RenderExpr>` — tracks which render expression each render source block has.

### E2ETransition

`crates/holon-integration-tests/src/pbt/transitions.rs` — the full set of mutations the PBT can apply:

- `StartApp` — initialize the system (must be first)
- `CreateBlock { parent_id, content, ... }`
- `UpdateBlock { id, content }`
- `DeleteBlock { id }`
- `IndentBlock { id }`
- `OutdentBlock { id }`
- `MoveBlock { id, new_parent_id }`
- `SetTaskState { id, state }`
- `WriteOrgFile { doc_id }` — write org file and verify round-trip
- `ApplyMutation { kind }` — generic mutation from generator
- `Navigate { direction }`

### Invariants

PBTs check invariants after each transition:
- inv1–inv5: block structure, parent-child consistency, document membership
- inv6: navigation state consistency
- inv7: org round-trip fidelity
- inv8: displayed blocks match `expected_focus_root_ids()`
- inv9: query results match reference model
- inv10 (a–f): display node structure assertions (decompiled from ViewModel)

### TestQuery & WatchSpec

`crates/holon-integration-tests/src/pbt/query.rs`:
```rust
pub struct TestQuery {
    pub language: QueryLanguage,
    pub source: String,
}
```

`TestQuery::to_prql()`, `to_sql()`, `to_gql()` — compile to different languages. `evaluate()` runs against the reference model.

`WatchSpec` — a query + region + render expression combination used to set up live watchers.

### Display Assertions

`crates/holon-integration-tests/src/display_assertions.rs` — decompiles the live `ViewModel` tree back to row data for comparison.

`extract_rendered_rows(tree)` — walks the ViewModel tree:
- `TableRow` → extracts `data` map
- `BlockRef` → extracts entity ID
- `Row` → extracts text content

inv10d: root widget matches expected `RenderExpr`
inv10e: entity ID set from display is subset of reference model IDs
inv10f: decompiled data matches query data from reference model

## Test Environment

`crates/holon-integration-tests/src/test_environment.rs` — `TestEnvironment` sets up a full Holon stack in-process for tests:
- Temp directory for org files and Loro snapshot
- Full `BackendEngine` with all schema modules
- Org sync controller
- `HolonService` for operations

`setup_watch(region, query, language)` — starts a live watcher and returns `WatchHandle`.

## UI Driver

`crates/holon-integration-tests/src/ui_driver.rs` — abstract UI operations for PBT:
- `navigate(direction)`
- `toggle_task_state(id)`
- `edit_content(id, content)`

## Polling

`crates/holon-integration-tests/src/polling.rs` — utilities for waiting for eventual-consistent state in async tests. `poll_until(condition, timeout)`.

## Always Tee Before Filtering

Per project rule: always pipe test output through `tee` before filtering:
```bash
cargo nextest run general_e2e_pbt 2>&1 | tee /tmp/pbt.log | grep FAIL
```

This preserves the full output (non-deterministic UUIDs, timing) for cross-referencing.

## Related Pages

- [[concepts/pbt-testing]] — PBT design rationale
- [[entities/holon-crate]] — the system being tested
- [[entities/holon-orgmode]] — org round-trip tested by `WriteOrgFile`
