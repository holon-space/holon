# General-Purpose Property-Based E2E Test

## Goal

Create a stateful property-based test that verifies:
1. **Convergence**: All persistence formats (Loro, Org files, future formats) converge to the same state
2. **CDC correctness**: Change streams accurately reflect mutations
3. **Multi-source consistency**: Mutations from UI, external files, and Loro sync all produce consistent states

## Design Principles

1. **Loro as canonical source** - All other formats sync from/to Loro
2. **Serialization abstraction** - Formats are pluggable, not hard-coded
3. **CDC-driven verification** - Use `query_and_watch()` instead of polling
4. **Unified mutations** - Single mutation model with `source` field
5. **Root document awareness** - `index.org` is just one instance of a document

## File Location

Create: `crates/holon/src/testing/general_e2e_pbt.rs`

Add to `crates/holon/src/testing/mod.rs`:
```rust
#[cfg(test)]
mod general_e2e_pbt;
```

## Architecture

### 1. Serialization Format Trait

```rust
/// Abstraction for any persistence format (Loro, Org, JSON, etc.)
pub trait SerializationFormat: Send + Sync {
    fn name(&self) -> &str;

    /// Read current state as blocks
    async fn read_state(&self) -> Result<Vec<Block>>;

    /// Apply a mutation directly to this format
    async fn apply_mutation(&self, mutation: &Mutation) -> Result<()>;

    /// Subscribe to external changes (file watcher, Loro sync, etc.)
    fn external_changes(&self) -> Option<BoxStream<'static, ExternalChange>>;

    /// Sync state from the canonical source (Loro)
    async fn sync_from_canonical(&self, blocks: &[Block]) -> Result<()>;
}

// Implementations:
// - LoroSerializationFormat (wraps LoroBackend)
// - OrgFileSerializationFormat (wraps OrgModeSyncProvider)
// - (future) JsonSerializationFormat, MarkdownSerializationFormat
```

### 2. Unified Mutation Model

Instead of separate transitions per source, use a unified model:

```rust
/// Source of a mutation
#[derive(Debug, Clone, PartialEq)]
pub enum MutationSource {
    /// User action via BackendEngine operations
    UI,
    /// External change to a serialization format (file edit, etc.)
    External { format: String },
    /// Incoming Loro sync from peer
    LoroSync { peer_id: String },
}

/// A mutation to the data model
#[derive(Debug, Clone)]
pub enum Mutation {
    Create {
        entity: String,
        id: String,
        parent_id: String,
        fields: HashMap<String, Value>,
    },
    Update {
        entity: String,
        id: String,
        fields: HashMap<String, Value>,
    },
    Delete {
        entity: String,
        id: String,
    },
    Move {
        entity: String,
        id: String,
        new_parent_id: String,
    },
}

/// A mutation event with source information
#[derive(Debug, Clone)]
pub struct MutationEvent {
    pub source: MutationSource,
    pub mutation: Mutation,
}
```

### 3. Reference Model

Single source of truth for the expected state:

```rust
/// A block in the reference model
#[derive(Debug, Clone, PartialEq)]
pub struct RefBlock {
    pub id: String,
    pub parent_id: String,
    pub content: String,
    pub properties: HashMap<String, Value>,
}

/// Reference state tracking all expected data
#[derive(Debug, Clone)]
pub struct ReferenceState {
    /// Canonical block state
    blocks: HashMap<String, RefBlock>,

    /// Root document ID (corresponds to index.org etc.)
    root_document_id: String,

    /// Expected CDC events not yet observed
    pending_cdc_events: VecDeque<ExpectedCDCEvent>,

    /// Active query watches (query_id -> filter predicate)
    active_watches: HashMap<String, WatchSpec>,

    /// ID counter for generating unique IDs
    next_id: usize,

    /// Runtime for async operations
    runtime: Arc<tokio::runtime::Runtime>,
}

/// Expected CDC event
#[derive(Debug, Clone)]
pub struct ExpectedCDCEvent {
    pub query_id: String,
    pub change_type: ChangeType,
    pub entity_id: String,
}

/// Specification for a watch
#[derive(Debug, Clone)]
pub struct WatchSpec {
    pub prql: String,
    pub filter: Option<FilterPredicate>,
}
```

### 4. Transitions

Only user-observable operations are transitions. Sync, CDC processing, and verification
happen automatically in the backend and `check_invariants`.

```rust
#[derive(Debug, Clone)]
pub enum E2ETransition {
    /// Apply a mutation from any source (UI, external file, Loro sync)
    ApplyMutation(MutationEvent),

    /// Set up a CDC watch for a query
    SetupWatch {
        query_id: String,
        prql: String,
        filter: Option<FilterPredicate>,
    },

    /// Remove a watch
    RemoveWatch { query_id: String },

    /// Switch the active view filter
    SwitchView { view_name: String },
}
```

### 5. Initial State Strategies

```rust
impl ReferenceStateMachine for ReferenceState {
    fn init_state() -> BoxedStrategy<Self::State> {
        prop_oneof![
            // Blank slate with just root document
            1 => Just(ReferenceState::empty()),

            // Pre-populated with random block tree
            2 => generate_block_tree(1..10).prop_map(ReferenceState::with_blocks),

            // Simulate existing document structure
            1 => generate_document_structure().prop_map(ReferenceState::from_structure),
        ].boxed()
    }
}

fn generate_block_tree(size: impl Into<SizeRange>) -> impl Strategy<Value = Vec<RefBlock>> {
    size.into().prop_flat_map(|size| {
        let mut blocks = vec![];
        let mut strategies = vec![];

        // Generate tree structure ensuring valid parent references
        for i in 0..size {
            let parent_ids: Vec<String> = if i == 0 {
                vec!["root".to_string()]
            } else {
                (0..i).map(|j| format!("block-{}", j)).collect()
            };

            strategies.push((
                prop::sample::select(parent_ids),
                "[a-zA-Z][a-zA-Z0-9 ]{0,30}",
            ).prop_map(move |(parent_id, content)| RefBlock {
                id: format!("block-{}", i),
                parent_id,
                content,
                properties: HashMap::new(),
            }));
        }

        strategies
    })
}
```

### 6. System Under Test

```rust
pub struct E2ESut {
    /// The BackendEngine for UI operations
    engine: Arc<BackendEngine>,

    /// Loro backend (canonical source)
    loro: Arc<LoroBackend>,

    /// Registered serialization formats
    formats: HashMap<String, Arc<dyn SerializationFormat>>,

    /// Active CDC watches (query_id -> stream)
    active_watches: HashMap<String, RowChangeStream>,

    /// UI model built from CDC events (query_id -> rows)
    ui_model: HashMap<String, Vec<HashMap<String, Value>>>,

    /// Current view filter
    current_view: String,

    /// Runtime for async operations
    runtime: Arc<tokio::runtime::Runtime>,
}

impl E2ESut {
    /// Create with default formats (Loro + Org)
    pub async fn new() -> Result<Self> {
        let loro = Arc::new(LoroBackend::new_in_memory().await?);
        let org = Arc::new(OrgFileSerializationFormat::new_temp().await?);

        let mut formats = HashMap::new();
        formats.insert("loro".to_string(), loro.clone() as Arc<dyn SerializationFormat>);
        formats.insert("org".to_string(), org as Arc<dyn SerializationFormat>);

        let engine = Arc::new(BackendEngine::new_with_loro(loro.clone()).await?);

        Ok(Self {
            engine,
            loro,
            formats,
            active_watches: HashMap::new(),
            ui_model: HashMap::new(),
            current_view: "all".to_string(),
            runtime: Arc::new(tokio::runtime::Runtime::new()?),
        })
    }

    /// Add a serialization format
    pub fn add_format(&mut self, name: &str, format: Arc<dyn SerializationFormat>) {
        self.formats.insert(name.to_string(), format);
    }
}
```

### 7. Transition Application

```rust
impl StateMachineTest for E2ESut {
    type SystemUnderTest = Self;
    type Reference = ReferenceState;

    fn apply(
        mut state: Self::SystemUnderTest,
        _ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: E2ETransition,
    ) -> Self::SystemUnderTest {
        let runtime = state.runtime.clone();

        runtime.block_on(async {
            match transition {
                E2ETransition::ApplyMutation(event) => {
                    state.apply_mutation(event).await;
                }

                E2ETransition::SetupWatch { query_id, prql, .. } => {
                    let (_spec, initial, stream) = state.engine
                        .query_and_watch(prql, HashMap::new())
                        .await
                        .expect("Watch setup failed");

                    state.ui_model.insert(query_id.clone(), initial);
                    state.active_watches.insert(query_id, stream);
                }

                E2ETransition::RemoveWatch { query_id } => {
                    state.active_watches.remove(&query_id);
                    state.ui_model.remove(&query_id);
                }

                E2ETransition::SwitchView { view_name } => {
                    state.current_view = view_name;
                }
            }

            // After every transition, drain CDC events to keep UI model in sync
            state.drain_cdc_events().await;
        });

        state
    }
}

impl E2ESut {
    async fn apply_mutation(&mut self, event: MutationEvent) {
        match event.source {
            MutationSource::UI => {
                // Route through BackendEngine operations
                let (entity, op, params) = event.mutation.to_operation();
                self.engine.execute_operation(&entity, &op, params).await.unwrap();
                // Backend automatically syncs to all formats
            }

            MutationSource::External { format } => {
                // Apply directly to the serialization format (simulates external edit)
                if let Some(fmt) = self.formats.get(&format) {
                    fmt.apply_mutation(&event.mutation).await.unwrap();
                }
                // Backend's file watcher detects change and syncs to Loro automatically
                // For testing, we trigger the sync explicitly
                self.engine.trigger_sync_from_format(&format).await.unwrap();
            }

            MutationSource::LoroSync { .. } => {
                // Apply directly to Loro (simulates incoming peer sync)
                self.loro.apply_mutation(&event.mutation).await.unwrap();
                // Backend detects Loro change and syncs to other formats automatically
            }
        }
    }

    /// Drain all pending CDC events and apply to UI model
    async fn drain_cdc_events(&mut self) {
        use tokio::time::{timeout, Duration};

        for (query_id, stream) in &mut self.active_watches {
            while let Ok(Some(event)) = timeout(
                Duration::from_millis(50),
                stream.next()
            ).await {
                if let Some(ui_data) = self.ui_model.get_mut(query_id) {
                    apply_cdc_event_to_model(ui_data, event);
                }
            }
        }
    }
}
```

### 8. CDC Event Application

```rust
fn apply_cdc_event_to_model(
    model: &mut Vec<HashMap<String, Value>>,
    event: RowChange,
) {
    match event.data {
        ChangeData::Created { data, .. } => {
            model.push(data);
        }
        ChangeData::Updated { id, data, .. } | ChangeData::FieldsChanged { id, data, .. } => {
            if let Some(row) = model.iter_mut().find(|r| r.get("id") == Some(&Value::String(id.clone()))) {
                for (k, v) in data {
                    row.insert(k, v);
                }
            }
        }
        ChangeData::Deleted { id, .. } => {
            model.retain(|r| r.get("id") != Some(&Value::String(id.clone())));
        }
    }
}
```

### 9. Transition Strategy Generation

```rust
impl ReferenceStateMachine for ReferenceState {
    fn transitions(state: &Self::State) -> BoxedStrategy<E2ETransition> {
        let block_ids: Vec<String> = state.blocks.keys().cloned().collect();
        let format_names: Vec<String> = vec!["org".to_string()]; // extensible
        let next_id = state.next_id;

        let mut strategies: Vec<BoxedStrategy<E2ETransition>> = Vec::new();

        // === Mutations with unified source ===

        // UI mutations
        strategies.push(
            generate_mutation(next_id, &block_ids)
                .prop_map(|mutation| E2ETransition::ApplyMutation(MutationEvent {
                    source: MutationSource::UI,
                    mutation,
                }))
                .boxed()
        );

        // External mutations (for each format)
        for format in &format_names {
            let fmt = format.clone();
            strategies.push(
                generate_mutation(next_id, &block_ids)
                    .prop_map(move |mutation| E2ETransition::ApplyMutation(MutationEvent {
                        source: MutationSource::External { format: fmt.clone() },
                        mutation,
                    }))
                    .boxed()
            );
        }

        // Loro sync mutations
        strategies.push(
            generate_mutation(next_id, &block_ids)
                .prop_map(|mutation| E2ETransition::ApplyMutation(MutationEvent {
                    source: MutationSource::LoroSync { peer_id: "peer-1".to_string() },
                    mutation,
                }))
                .boxed()
        );

        // === Watch management ===

        strategies.push(
            generate_watch_setup()
                .prop_map(|(query_id, prql)| E2ETransition::SetupWatch {
                    query_id,
                    prql,
                    filter: None,
                })
                .boxed()
        );

        if !state.active_watches.is_empty() {
            let watch_ids: Vec<String> = state.active_watches.keys().cloned().collect();
            strategies.push(
                prop::sample::select(watch_ids)
                    .prop_map(|query_id| E2ETransition::RemoveWatch { query_id })
                    .boxed()
            );
        }

        // === View switching ===

        strategies.push(
            prop::sample::select(vec!["all".to_string(), "sidebar".to_string(), "main".to_string()])
                .prop_map(|view_name| E2ETransition::SwitchView { view_name })
                .boxed()
        );

        prop::strategy::Union::new(strategies).boxed()
    }
}

fn generate_mutation(next_id: usize, existing_ids: &[String]) -> impl Strategy<Value = Mutation> {
    let create = (
        "[a-zA-Z][a-zA-Z0-9 ]{0,20}",
        prop::sample::select(vec!["sidebar".to_string(), "main".to_string()]),
    ).prop_map(move |(content, region)| Mutation::Create {
        entity: "blocks".to_string(),
        id: format!("block-{}", next_id),
        parent_id: "root".to_string(),
        fields: [
            ("content".to_string(), Value::String(content)),
            ("region".to_string(), Value::String(region)),
        ].into_iter().collect(),
    });

    if existing_ids.is_empty() {
        return create.boxed();
    }

    let ids = existing_ids.to_vec();
    let update = (
        prop::sample::select(ids.clone()),
        "[a-zA-Z][a-zA-Z0-9 ]{0,20}",
    ).prop_map(|(id, new_content)| Mutation::Update {
        entity: "blocks".to_string(),
        id,
        fields: [("content".to_string(), Value::String(new_content))].into_iter().collect(),
    });

    let delete = prop::sample::select(ids)
        .prop_map(|id| Mutation::Delete {
            entity: "blocks".to_string(),
            id,
        });

    prop_oneof![
        3 => create,
        2 => update,
        1 => delete,
    ].boxed()
}
```

### 10. Invariant Checking (includes convergence verification)

```rust
impl StateMachineTest for E2ESut {
    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        let runtime = state.runtime.clone();

        runtime.block_on(async {
            // 1. Loro matches reference model
            let loro_blocks = state.loro.get_all_blocks().await.unwrap();
            let ref_blocks: Vec<_> = ref_state.blocks.values().cloned().collect();
            assert_blocks_equivalent(&loro_blocks, &ref_blocks, "Loro diverged from reference");

            // 2. All serialization formats converged to Loro
            for (name, format) in &state.formats {
                if name == "loro" { continue; }

                let format_blocks = format.read_state().await.unwrap();
                assert_blocks_equivalent(
                    &format_blocks,
                    &loro_blocks,
                    &format!("Format '{}' diverged from Loro", name)
                );
            }

            // 3. UI model (built from CDC) matches reference
            for (query_id, ui_data) in &state.ui_model {
                if let Some(watch_spec) = ref_state.active_watches.get(query_id) {
                    let expected = ref_state.query_results(&watch_spec);
                    assert_eq!(
                        ui_data.len(),
                        expected.len(),
                        "UI model for '{}' has wrong count",
                        query_id
                    );
                }
            }

            // 4. View selection synchronized
            assert_eq!(state.current_view, ref_state.current_view());

            // 5. Active watches match
            assert_eq!(
                state.active_watches.keys().collect::<HashSet<_>>(),
                ref_state.active_watches.keys().collect::<HashSet<_>>(),
                "Watch sets diverged"
            );

            // 6. Structural integrity: no orphan blocks
            for block in &loro_blocks {
                if block.parent_id != "root" {
                    assert!(
                        loro_blocks.iter().any(|b| b.id == block.parent_id),
                        "Orphan block: {} has invalid parent {}",
                        block.id,
                        block.parent_id
                    );
                }
            }
        });
    }
}
```

## Key Invariants (checked after every transition)

1. **Loro-Reference Equivalence**: Loro state always matches reference model
2. **Format Convergence**: All serialization formats agree with Loro
3. **CDC Completeness**: UI model (built from CDC events) matches reference query results
4. **View Synchronization**: SUT and reference have same current view
5. **Watch Consistency**: Active watches in SUT match reference
6. **Structural Integrity**: No orphan blocks, all parents exist or are root

## Adding New Serialization Formats

1. Implement `SerializationFormat` trait:

```rust
pub struct JsonSerializationFormat {
    file_path: PathBuf,
}

impl SerializationFormat for JsonSerializationFormat {
    fn name(&self) -> &str { "json" }

    async fn read_state(&self) -> Result<Vec<Block>> {
        let content = tokio::fs::read_to_string(&self.file_path).await?;
        let blocks: Vec<Block> = serde_json::from_str(&content)?;
        Ok(blocks)
    }

    async fn apply_mutation(&self, mutation: &Mutation) -> Result<()> {
        let mut blocks = self.read_state().await?;
        mutation.apply_to(&mut blocks);
        let content = serde_json::to_string_pretty(&blocks)?;
        tokio::fs::write(&self.file_path, content).await?;
        Ok(())
    }

    fn external_changes(&self) -> Option<BoxStream<'static, ExternalChange>> {
        // File watcher implementation
        None
    }

    async fn sync_from_canonical(&self, blocks: &[Block]) -> Result<()> {
        let content = serde_json::to_string_pretty(&blocks)?;
        tokio::fs::write(&self.file_path, content).await?;
        Ok(())
    }
}
```

2. Register in test setup:

```rust
let json_format = Arc::new(JsonSerializationFormat::new_temp().await?);
sut.add_format("json", json_format);
```

3. Add to format_names in transition generation.

## Running the Test

```bash
cargo test -p holon general_e2e_pbt -- --nocapture
```

## Future Extensions

1. **Conflict Resolution**: Add concurrent mutations from multiple sources, verify CRDT resolution
2. **Network Partitions**: Simulate Loro disconnection/reconnection, verify eventual consistency
3. **Undo/Redo**: Add `Undo` / `Redo` transitions, verify state restoration
4. **Schema Evolution**: Test migrations when block schema changes
5. **Large Scale**: Test with 1000+ blocks, verify performance doesn't degrade

## Dependencies

Already in `Cargo.toml`:
```toml
proptest = "1.4"
proptest-state-machine = "0.3"
```
