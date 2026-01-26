# Handoff: Extract PBT State Machine into Library (Phase 2)

## Goal

Move the core PBT state machine from `crates/holon-integration-tests/tests/general_e2e_pbt.rs` (a test file, invisible to other crates) into `crates/holon-integration-tests/src/` (library) so that:

1. The backend PBT test (`general_e2e_pbt.rs`) continues working unchanged
2. The Flutter FFI (`frontends/flutter/rust/src/api/shared_pbt.rs`) can instantiate `E2ESut` with a `FlutterMutationDriver` and run the same state machine

## Current State (Phase 1 Complete)

### What exists

```
crates/holon-integration-tests/src/
├── mutation_driver.rs          # NEW: MutationDriver trait + DirectMutationDriver
├── lib.rs                      # Exports MutationDriver, DirectMutationDriver
├── assertions.rs, org_utils.rs, polling.rs, test_environment.rs, widget_state.rs

crates/holon-integration-tests/tests/
├── general_e2e_pbt.rs          # 4000 lines — ALL state machine code lives here

frontends/flutter/rust/src/api/
├── flutter_mutation_driver.rs  # NEW: FlutterMutationDriver (DartFnFuture callback)
├── shared_pbt.rs               # NEW: run_shared_pbt() — Phase 1 smoke test only
```

### What works

- `E2ESut.driver: Option<Box<dyn MutationDriver>>` — installed after `start_app`
- Two `execute_op` call sites in `apply_mutation()` and `ConcurrentMutations` replaced with `driver.apply_ui_mutation()`
- `DirectMutationDriver` wraps `Arc<BackendEngine>` and calls `execute_operation`
- `FlutterMutationDriver` serializes params to JSON and calls DartFnFuture callback
- `run_shared_pbt()` FFI works (FRB codegen generates Dart binding in `shared_pbt.dart`)
- Phase 1 smoke test: create/update/delete via MutationDriver callback

### What's missing

`run_shared_pbt()` currently runs a trivial 3-step smoke test. It needs to run the actual proptest state machine — but `E2ESut`, `ReferenceState`, `E2ETransition`, etc. are all in `general_e2e_pbt.rs` (a test file, not importable from the Flutter crate).

## What to Extract

### File structure (suggested)

```
crates/holon-integration-tests/src/
├── pbt/
│   ├── mod.rs                  # Re-exports
│   ├── types.rs                # MutationSource, Mutation, MutationEvent, TestVariant, VariantMarker, Full, SqlOnly
│   ├── reference_state.rs      # ReferenceState, ExpectedCDCEvent, WatchSpec, NavigationHistory, LayoutBlockInfo
│   ├── transitions.rs          # E2ETransition enum
│   ├── generators.rs           # proptest Strategy functions (generate_mutation, generate_org_file_content, etc.)
│   ├── sut.rs                  # E2ESut struct + apply_mutation + concurrent mutations
│   ├── state_machine.rs        # VariantRef, ReferenceStateMachine impl, StateMachineTest impl
│   ├── invariants.rs           # check_invariants, find_document_for_block
│   ├── crdt.rs                 # loro_merge_text
│   └── query.rs                # TestQuery, TestPredicate, value_to_sql_literal, etc.
```

### Line ranges in general_e2e_pbt.rs (4000 lines)

| Section | Lines | Target file |
|---------|-------|-------------|
| CRDT merge simulation | 67–112 | `crdt.rs` |
| Mutation model (MutationSource, Mutation, MutationEvent) | 114–477 | `types.rs` |
| Test variant config (TestVariant, VariantMarker, Full, SqlOnly) | 478–531 | `types.rs` |
| Layout block classification (LayoutBlockInfo) | 536–576 | `reference_state.rs` |
| Query representation (TestQuery, TestPredicate, helpers) | 577–811 | `query.rs` |
| Reference state (ReferenceState, constants, impl) | 812–1149 | `reference_state.rs` |
| Transitions (E2ETransition enum) | 1150–1268 | `transitions.rs` |
| E2ESut struct + Deref + helpers | 1269–1332 | `sut.rs` |
| Reference state machine (VariantRef, ReferenceStateMachine impl) | 1333–2541 | `state_machine.rs` |
| StateMachineTest impl (init_test, apply, check_invariants) | 2542–3949 | `state_machine.rs` + `sut.rs` |
| Generator functions | 2264–2698 (inside ReferenceStateMachine) | `generators.rs` |
| Invariant helpers | 3950–3975 | `invariants.rs` |
| Test entry points (pbt_config, prop_state_machine! macros) | 3976–4000 | Stays in test file |

### What stays in `general_e2e_pbt.rs`

Only the thin test entry points:

```rust
use holon_integration_tests::pbt::*;

proptest_state_machine::prop_state_machine! {
    #![proptest_config(pbt_config())]
    #[test]
    fn general_e2e_pbt(sequential 3..20 => E2ESut<Full>);
}

proptest_state_machine::prop_state_machine! {
    #![proptest_config(pbt_config())]
    #[test]
    fn general_e2e_pbt_sql_only(sequential 3..20 => E2ESut<SqlOnly>);
}
```

## Key Constraints

### 1. Dependencies must move from dev-dependencies to dependencies

Currently `general_e2e_pbt.rs` uses these as dev-deps:
- `proptest`, `proptest-state-machine` — needed for `Strategy`, `ReferenceStateMachine`, `StateMachineTest`
- `loro` — for CRDT merge simulation
- `similar-asserts` — for diff-friendly assertions
- `serde_json` — for properties
- `regex` — unused in PBT, only in cucumber

These would need to become regular dependencies of `holon-integration-tests` (behind a feature flag like `pbt`).

### 2. `StateMachineTest` impl references `E2ESut` (SUT = Self)

The `StateMachineTest for E2ESut<V>` impl's `init_test()` creates an `E2ESut::new(runtime)`. For Flutter, we need a different `init_test()` that:
- Creates E2ESut with a pre-installed `FlutterMutationDriver`
- Doesn't create its own tokio Runtime (Flutter already has one)

**Approach**: Make `E2ESut::new()` take an optional driver. Or: `E2ESut::with_driver(runtime, driver)`.

### 3. `prop_state_machine!` macro constraint

The macro generates: `fn init_test(ref_state) -> Self`. This means the driver must be constructible without parameters, OR:
- Use a thread-local/static to pass the driver to `init_test`
- Have `init_test` default to `DirectMutationDriver` and provide a separate entry point for Flutter

### 4. Flutter can't use `prop_state_machine!` macro

Flutter needs a manual runner (like the existing `flutter_pbt_runner.rs` pattern) that:
1. Creates `ReferenceState`
2. Generates transitions via `ReferenceStateMachine::transitions()`
3. Checks preconditions
4. Applies to reference + SUT
5. Checks invariants

This is ~50 lines of loop code, already implemented in `flutter_pbt_runner.rs:50-161`.

### 5. Cargo.toml structure

```toml
[features]
pbt = [
    "dep:proptest",
    "dep:proptest-state-machine",
    "dep:loro",
    "dep:similar-asserts",
    "dep:serde_json",
]

[dependencies]
proptest = { workspace = true, optional = true }
proptest-state-machine = { workspace = true, optional = true }
loro = { workspace = true, optional = true }
similar-asserts = { version = "1.7.0", optional = true }
serde_json = { workspace = true, optional = true }
```

The Flutter crate adds: `holon-integration-tests = { path = "...", features = ["pbt"] }`

## Suggested Implementation Order

1. **Create `pbt` feature flag** in `holon-integration-tests/Cargo.toml`
2. **Move types first** (`types.rs`, `query.rs`) — least dependencies
3. **Move `ReferenceState`** and its impl — depends on types + query
4. **Move `E2ETransition`** — depends on types + reference_state
5. **Move generators** — depends on everything above + proptest
6. **Move `E2ESut`** — depends on MutationDriver + TestContext
7. **Move `StateMachineTest` impl** — depends on E2ESut + ReferenceState
8. **Update `general_e2e_pbt.rs`** to import from library
9. **Update `shared_pbt.rs`** to use extracted types + manual runner
10. **Verify**: `cargo test -p holon-integration-tests -- general_e2e_pbt`

## FRB Gotcha

FRB cannot generate Dart bindings for functions with `DartFnFuture` params if they live in `ffi_bridge.rs`. The function must be in a **separate file** (like `shared_pbt.rs`). This is because FRB's parser fails with `error=when trying to parse DartFn` when the function coexists with non-callback functions in the same file. The `rust_preamble` in `flutter_rust_bridge.yaml` includes `use flutter_rust_bridge::DartFnFuture;` to fix a related codegen bug where generated code uses bare `DartFnFuture` without the fully-qualified path.
