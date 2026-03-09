# Handoff: Test Infrastructure Unification - COMPLETE

## Summary

All test infrastructure has been unified. `TestContext` is the single source of truth for test state. `E2ESut` is a thin newtype wrapper that satisfies Rust's orphan rules while providing transparent access via `Deref`/`DerefMut`.

## Final Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    TestContext                               │
│  ┌─────────────────────────────────────────────────────────┐│
│  │ Core fields:                                            ││
│  │ • ctx: E2ETestContext                                   ││
│  │ • doc_store: Arc<RwLock<LoroDocumentStore>>             ││
│  │ • temp_dir: TempDir                                     ││
│  │ • documents: HashMap<String, PathBuf>                   ││
│  │ • runtime: Arc<Runtime>                                 ││
│  │                                                          ││
│  │ CDC tracking fields:                                    ││
│  │ • active_watches: HashMap<String, RowChangeStream>      ││
│  │ • ui_model: HashMap<String, Vec<...>>                   ││
│  │ • current_view: String                                  ││
│  │ • region_streams: HashMap<String, RowChangeStream>      ││
│  │ • region_data: HashMap<String, Vec<...>>                ││
│  │                                                          ││
│  │ All methods available to both Cucumber and PBT tests    ││
│  └─────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────┘
                              ▲
                              │ newtype wrapper (Deref/DerefMut)
                              │
┌─────────────────────────────────────────────────────────────┐
│              E2ESut (newtype for orphan rules)              │
│  ┌─────────────────────────────────────────────────────────┐│
│  │ pub struct E2ESut(pub TestContext);                     ││
│  │                                                          ││
│  │ impl Deref<Target = TestContext>                        ││
│  │ impl DerefMut                                           ││
│  │                                                          ││
│  │ PBT-specific methods:                                   ││
│  │ • apply_navigation(&mut self, ...)                      ││
│  │ • apply_mutation(&mut self, ...)                        ││
│  │ • apply_external_mutation(&mut self, ...)               ││
│  └─────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────┘
```

## Key Design Decisions

### Why a Newtype Wrapper?

Rust's orphan rules prevent implementing `StateMachineTest` (from proptest_state_machine) directly for `TestContext` (from our crate). The newtype wrapper `E2ESut(TestContext)` satisfies the orphan rules while `Deref`/`DerefMut` provides transparent access to all TestContext methods.

### What Lives Where?

| Functionality | Location | Reason |
|---------------|----------|--------|
| Core test state | TestContext | Shared by Cucumber and PBT |
| CDC tracking | TestContext | Useful for both test types |
| Document operations | TestContext | Common test utilities |
| Block CRUD | TestContext | `create_block`, `create_source_block`, `update_block_content`, `delete_block` |
| Navigation operations | TestContext | `navigate_focus`, `navigate_back`, `navigate_forward`, `navigate_home` |
| Watch operations | TestContext | `setup_watch`, `remove_watch` |
| View operations | TestContext | `switch_view` |
| Polling helpers | TestContext | `wait_for_block`, `wait_for_block_count`, `simulate_restart` |
| `apply_mutation` | E2ESut | Uses PBT types (MutationEvent, ReferenceState) |
| `StateMachineTest` impl | E2ESut | Orphan rules require newtype |

## Usage

### Cucumber Tests (thin step definitions)

```rust
// Step definitions are now one-liners
#[when(expr = "I create a block with id {string} and content {string} in document {string}")]
async fn create_block(world: &mut HolonWorld, block_id: String, content: String, file_name: String) {
    let doc_uri = format!("holon-doc://{}", file_name);
    world.ctx().create_block(&block_id, &doc_uri, &content).await.unwrap();
}

#[when(expr = "I update block {string} content to {string}")]
async fn update_block(world: &mut HolonWorld, block_id: String, new_content: String) {
    world.ctx().update_block_content(&block_id, &new_content).await.unwrap();
}

#[then(expr = "the block {string} should exist in the database")]
async fn block_should_exist(world: &mut HolonWorld, block_id: String) {
    let exists = world.ctx().wait_for_block(&block_id, Duration::from_secs(5)).await;
    assert!(exists);
}
```

### PBT Tests (thin dispatch in StateMachineTest::apply)

```rust
match &transition {
    E2ETransition::NavigateFocus { region, block_id } => {
        state.navigate_focus(region, block_id).await?;
    }
    E2ETransition::SetupWatch { query_id, prql, .. } => {
        state.setup_watch(query_id, prql).await?;
    }
    E2ETransition::SimulateRestart => {
        state.simulate_restart(ref_state.blocks.len()).await?;
    }
    // PBT-specific methods (depend on ReferenceState)
    E2ETransition::ApplyMutation(event) => {
        state.apply_mutation(event.clone(), &ref_state).await;
    }
}
```

## Files Modified

| File | Changes |
|------|---------|
| `src/test_context.rs` | Added CDC tracking fields and drain methods |
| `tests/general_e2e_pbt.rs` | E2ESut is now a newtype wrapper with Deref |
| `Cargo.toml` | Added `futures` to test-infra features |

## Tests Verified

- `test_external_mutation_race_condition` - PASSED
- Build passes with only warnings about unused fields in ReferenceState

## Benefits

1. **Single source of truth** - All state in TestContext
2. **Transparent access** - E2ESut derefs to TestContext
3. **Cucumber can use CDC** - `drain_cdc_events()` and related fields available
4. **Minimal wrapper** - E2ESut is just ~20 lines of boilerplate
5. **Type safety** - Newtype satisfies orphan rules properly
