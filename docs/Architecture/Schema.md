# Schema & Entity Type System

*Part of [Architecture](../Architecture.md)*

## Schema Module System

Database objects (tables, views, materialized views) have complex dependencies. A materialized view depends on the tables it queries; views may depend on other views. Creating them in the wrong order causes failures. The Schema Module system provides declarative lifecycle management with automatic dependency ordering.

### SchemaModule Trait

Each logical group of database objects implements `SchemaModule`:

```rust
#[async_trait]
pub trait SchemaModule: Send + Sync {
    /// Unique name for logging and error messages
    fn name(&self) -> &str;

    /// Resources this module creates (tables, views, materialized views)
    fn provides(&self) -> Vec<Resource>;

    /// Resources this module depends on
    fn requires(&self) -> Vec<Resource>;

    /// Execute DDL to create/update schema objects (idempotent)
    async fn ensure_schema(&self, backend: &TursoBackend) -> Result<()>;

    /// Optional post-schema initialization (e.g., seed data)
    async fn initialize_data(&self, _backend: &TursoBackend) -> Result<()> {
        Ok(())
    }
}
```

### Resource Type

Resources represent database objects that can be provided or required:

```rust
pub enum Resource {
    Schema(String),      // Tables, views, materialized views
    Capability(String),  // Abstract capabilities
}

impl Resource {
    pub fn schema(name: &str) -> Self { Resource::Schema(name.to_string()) }
}
```

### Concrete Schema Modules

The system includes these core modules:

| Module | Provides | Requires |
|--------|----------|----------|
| `CoreSchemaModule` | `blocks`, `documents`, `directories` | (none) |
| `BlockHierarchySchemaModule` | `blocks_with_paths` | `blocks` |
| `NavigationSchemaModule` | `navigation_history`, `navigation_cursor`, `current_focus` | (none) |
| `SyncStateSchemaModule` | `sync_states` | (none) |
| `OperationsSchemaModule` | `operations` | (none) |
| Graph EAV schema (inline DDL) | `nodes`, `edges`, `node_labels`, `property_keys`, `*_props_*` | (none) |

**Runtime-defined types**: User-defined entity types (Person, Book, Organization) will generate `SchemaModule` implementations dynamically at startup. Each type becomes a module that provides its extension table (e.g., `person`) and requires `blocks`. The existing topological sort handles this naturally — user-defined type modules are registered alongside built-in modules. See [Entity Type System](#entity-type-system-partially-implemented).

Example implementation:

```rust
pub struct BlockHierarchySchemaModule;

#[async_trait]
impl SchemaModule for BlockHierarchySchemaModule {
    fn name(&self) -> &str { "block_hierarchy" }

    fn provides(&self) -> Vec<Resource> {
        vec![Resource::schema("blocks_with_paths")]
    }

    fn requires(&self) -> Vec<Resource> {
        vec![Resource::schema("blocks")]  // Must exist before this view
    }

    async fn ensure_schema(&self, backend: &TursoBackend) -> Result<()> {
        backend.execute_ddl(r#"
            CREATE MATERIALIZED VIEW IF NOT EXISTS blocks_with_paths AS
            WITH RECURSIVE paths AS (
                SELECT id, parent_id, content, '/' || id as path
                FROM blocks
                WHERE parent_id LIKE 'holon-doc://%'
                   OR parent_id = '__no_parent__'
                UNION ALL
                SELECT b.id, b.parent_id, b.content, p.path || '/' || b.id
                FROM blocks b
                INNER JOIN paths p ON b.parent_id = p.id
            )
            SELECT * FROM paths
        "#).await
    }
}
```

### SchemaRegistry

The registry collects modules and initializes them in dependency order:

```rust
pub struct SchemaRegistry {
    modules: Vec<Arc<dyn SchemaModule>>,
}

impl SchemaRegistry {
    pub fn register(&mut self, module: Arc<dyn SchemaModule>);

    /// Initialize all modules in topological order
    pub async fn initialize_all(
        &self,
        backend: Arc<RwLock<TursoBackend>>,
        scheduler_handle: &SchedulerHandle,
        pre_available: Vec<Resource>,
    ) -> Result<(), SchemaRegistryError>;
}
```

### Topological Sort

The registry builds a dependency DAG and uses Kahn's algorithm:

```
                    ┌─────────────────┐
                    │ CoreSchemaModule│
                    │ provides: blocks│
                    └────────┬────────┘
                             │
              requires: blocks
                             │
                             ▼
               ┌─────────────────────────┐
               │ BlockHierarchySchemaModule│
               │ provides: blocks_with_paths│
               └─────────────────────────┘
```

1. Build provider map: `Resource → module index`
2. Compute in-degrees for each module
3. Process modules with in-degree 0 first
4. After processing, mark provided resources as available
5. Decrement in-degrees of dependent modules
6. Repeat until all modules processed

### Error Handling

```rust
pub enum SchemaRegistryError {
    /// Circular dependency detected
    CycleDetected(String),

    /// Module requires a resource no module provides
    MissingDependency { module: String, resource: String },

    /// DDL execution or data initialization failed
    InitializationFailed { module: String, error: String },
}
```

### Integration with DI

During application startup in `create_backend_engine()`:

```rust
// 1. Create TursoBackend and DatabaseActor
let backend = Arc::new(RwLock::new(TursoBackend::new(db_path).await?));
let (actor, db_handle) = DatabaseActor::new(backend.clone()).await?;
tokio::spawn(actor.run());

// 2. Create OperationScheduler for dependency tracking
let (scheduler, scheduler_handle) = OperationScheduler::new(db_handle.clone());
tokio::spawn(scheduler.run());

// 3. Register DI services
register_core_services_with_backend(&mut services, db_path, backend.clone(), db_handle)?;

// 4. Initialize all schemas via registry (replaces manual mark_available calls)
let registry = create_core_schema_registry();
registry.initialize_all(backend.clone(), &scheduler_handle, vec![]).await?;

// 5. Build DI container and resolve BackendEngine
let provider = services.build();
let engine = Resolver::get_required::<BackendEngine>(&provider);
```

### Factory Function

```rust
/// Creates a SchemaRegistry with all core modules registered
pub fn create_core_schema_registry() -> SchemaRegistry {
    let mut registry = SchemaRegistry::new();
    registry.register(Arc::new(CoreSchemaModule));
    registry.register(Arc::new(BlockHierarchySchemaModule));
    registry.register(Arc::new(NavigationSchemaModule));
    registry.register(Arc::new(SyncStateSchemaModule));
    registry.register(Arc::new(OperationsSchemaModule));
    registry
}
```

### Adding New Schema Objects

To add a new table or view:

1. **Create a SchemaModule** in `storage/schema_modules.rs`:
   ```rust
   pub struct MyNewSchemaModule;

   #[async_trait]
   impl SchemaModule for MyNewSchemaModule {
       fn name(&self) -> &str { "my_new_schema" }
       fn provides(&self) -> Vec<Resource> { vec![Resource::schema("my_table")] }
       fn requires(&self) -> Vec<Resource> { vec![] }  // or dependencies
       async fn ensure_schema(&self, backend: &TursoBackend) -> Result<()> {
           backend.execute_ddl("CREATE TABLE IF NOT EXISTS my_table (...)").await
       }
   }
   ```

2. **Register in factory**:
   ```rust
   pub fn create_core_schema_registry() -> SchemaRegistry {
       let mut registry = SchemaRegistry::new();
       // ... existing modules ...
       registry.register(Arc::new(MyNewSchemaModule));
       registry
   }
   ```

3. **Export from `storage/mod.rs`** if needed externally.

The registry automatically determines the correct initialization order.

### Key Files

| Path | Description |
|------|-------------|
| `crates/holon/src/storage/schema_module.rs` | `SchemaModule` trait, `SchemaRegistry`, topological sort |
| `crates/holon/src/storage/schema_modules.rs` | Concrete module implementations |
| `crates/holon/src/storage/resource.rs` | `Resource` enum |
| `crates/holon/src/di/mod.rs` | Integration with DI and startup |

## Value Types

```rust
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(DateTime<Utc>),
    Json(serde_json::Value),
    Null,
}

pub type StorageEntity = HashMap<String, Value>;
```

## Schema System

```rust
pub struct TypeDefinition {
    pub name: String,                          // Entity/table name
    pub default_lifetime: FieldLifetime,
    pub fields: Vec<FieldSchema>,
    pub primary_key: String,                   // Defaults to "id"
    pub id_references: Option<String>,         // FK constraint for extension tables
    pub graph_label: Option<String>,           // GQL node label
    pub source: TypeSource,                    // BuiltIn or Runtime
}

pub struct FieldSchema {
    pub name: String,
    pub data_type: DataType,
    pub indexed: bool,
    pub primary_key: bool,
    pub nullable: bool,
}

pub trait IntoEntity {
    fn to_entity(&self) -> DynamicEntity;
    fn type_definition() -> TypeDefinition;
}

pub trait TryFromEntity: Sized {
    fn from_entity(entity: DynamicEntity) -> Result<Self>;
}
```

Auto-generates CREATE TABLE and CREATE INDEX SQL from `TypeDefinition`. The `#[derive(Entity)]` macro generates `IntoEntity` and `TryFromEntity` implementations for built-in types (Block, Document). User-defined types use YAML definitions that produce `TypeDefinition` at runtime. Both coexist — they produce `SchemaModule` implementations with the same table/index conventions. See [Entity Type System](#entity-type-system-partially-implemented).

## Entity Type System (Partially Implemented)

Holon supports **runtime-defined typed entities** — user-definable types like Person, Book, Organization — with typed fields, computed expressions, and cross-system identity. This extends the block model without replacing it.

### Design Principles

1. **Blocks remain the universal identity layer.** Every typed entity IS a block — it has a row in the `block` table for tree structure, links, content, and text. Extension tables add typed fields via JOIN on `id`.
2. **Types are defined at runtime.** No recompile needed. Type definitions are stored as data in Loro, projected to YAML files, and materialized as Turso DDL.
3. **Turso remains a pure cache.** Deleting the entire Turso database loses no data. Everything reconstructs from Loro (or from org/YAML files if Loro is also gone).
4. **Structural primacy.** The type system is structural intelligence — it works without AI, survives model swaps, and compounds value as entity density grows.

### Field Lifetimes

Each field in a type definition has a `lifetime` that determines where it is stored, whether it participates in CRDT merge, and how it is reconstructed:

```rust
enum FieldLifetime {
    /// Stored in Loro, projected to org/YAML, materialized to Turso.
    /// Survives any cache wipe. Participates in CRDT merge.
    Persistent,

    /// Derived from other fields via a Rhai expression. Turso only.
    /// Not stored in Loro or files. Recomputed on reconstruction.
    /// Subsumes the current prototype block `=`-prefixed expressions.
    Computed { expr: String },

    /// Turso only. Not in Loro, not in files. Device-local.
    /// Re-fetched from Digital Twin source on next sync cycle.
    /// NULL after cache reconstruction.
    Transient,

    /// Append-only time series. Survives cache wipe via separate backup.
    /// Not in Loro (no merge semantics needed). Not in org files.
    /// Queryable in Turso for historical analysis.
    Historical,
}
```

Propagation rules:

| Lifetime | Loro | Org/YAML | Turso | CRDT merge | Reconstruction |
|---|---|---|---|---|---|
| `Persistent` | Yes | Yes | Yes | Yes | From Loro |
| `Computed` | No | No | Yes | No (derived) | Recompute from persistent fields |
| `Transient` | No | No | Yes | No (device-local) | Re-fetch from DT source |
| `Historical` | No | No | Yes + backup | No | From backup |

### Type Definitions

Type definitions are stored in Loro as structured maps and bidirectionally projected to YAML files:

```
assets/default/
  index.org              # document tree, text content
  types/
    person.yaml          # type definition
    book.yaml
    organization.yaml
```

Example type definition:

```yaml
name: person
fields:
  email:            { type: text, lifetime: persistent, indexed: true }
  organization:     { type: ref, lifetime: persistent, target: organization }
  role:             { type: text, lifetime: persistent }
  display_name:
    type: text
    lifetime: computed
    expr: "first_name + ' ' + last_name"
  current_location: { type: text, lifetime: transient }
  energy:           { type: real, lifetime: transient }
```

**Sync**: A `TypeSyncController` mirrors the existing `OrgSyncController` pattern — bidirectional sync between Loro and YAML files with echo-suppression via `last_projection` comparison.

**Loro representation**: Type definitions live under a `types/` key in the LoroDoc as nested LoroMaps. Field names are map keys; field metadata (type, lifetime, expr, indexed, etc.) are nested maps.

### Extension Tables

Each entity type gets a Turso table that extends the universal `block` table:

```
┌─────────────────────────────────────────────────┐
│  block table (universal)                         │
│  id, content, parent_id, content_type, ...       │
│  Every entity has a row here.                    │
├───────────────┬───────────────┬─────────────────┤
│  person       │  book         │  organization    │
│  (extension)  │  (extension)  │  (extension)     │
│  email        │  author       │  domain          │
│  role         │  year         │  industry        │
│  org_id       │  rating       │  size            │
│  location*    │               │                  │
│  energy*      │               │                  │
│  (* transient)│               │                  │
└───────────────┴───────────────┴─────────────────┘
```

Generated DDL from type definitions:

```sql
CREATE TABLE IF NOT EXISTS person (
    id TEXT PRIMARY KEY REFERENCES block(id),
    email TEXT,
    organization TEXT,
    role TEXT,
    display_name TEXT,        -- computed: populated by trigger
    current_location TEXT,    -- transient: NULL after reconstruction
    energy REAL               -- transient: NULL after reconstruction
);
CREATE INDEX IF NOT EXISTS idx_person_email ON person(email);
```

**Queries** join naturally:

```prql
from block
join person [==id]
filter role == "Engineering Lead"
select {block.content, person.email, person.role}
```

**Schema evolution**: Adding a field = update type definition + `ALTER TABLE ADD COLUMN`. Removing a field = update type definition + drop column from extension table (data stays in Loro properties — no data loss). Renaming = add new + migrate + drop old, standard DDL.

**Schema Module integration**: Each runtime-defined type generates a `SchemaModule` implementation that provides its extension table and requires the `block` table. The existing dependency-ordering infrastructure in `schema_modules.rs` handles this — user-defined type modules are registered alongside the built-in modules.

### Instance Data

Instance data for typed entities lives in the block's properties in Loro — the same `properties` map that already holds freeform org properties like `collapse-to` and `column-order`. The type schema declares which property keys are "typed" (materialized to extension table columns) and which remain freeform (stay in the JSON `properties` column on the `block` table).

In org files, typed properties appear as standard org properties on headings:

```org
* Sarah Chen
:PROPERTIES:
:type: person
:email: sarah@example.com
:organization: [[Acme Corp]]
:role: Engineering Lead
:END:

Notes from our last conversation...
```

The `type: person` property links the block to its type definition. On cache reconstruction, the materializer reads the type, looks up the schema, and populates the `person` extension table with the declared persistent fields.

### Reconstruction Guarantee

After a Turso wipe, the startup sequence is:

1. **Load type definitions** from Loro → generate `CREATE TABLE` DDL for each type → execute
2. **Load blocks** from Loro → `INSERT INTO block` (existing logic)
3. **Populate extension tables**: for each block with a `type` property, read its properties → `INSERT INTO {type}` with persistent fields only
4. **Recompute computed fields**: evaluate Rhai expressions for each row, populate computed columns
5. **Create materialized views, indexes** (existing logic)
6. **Transient fields**: left NULL — Digital Twin sync fills them on next poll/webhook cycle
7. **Historical fields**: restored from separate backup if available

Steps 1, 3, and 4 are new. The rest is the existing startup sequence.

### Confirmation-Driven Edge Creation

The Integrator AI role proposes typed relationships between entities for human confirmation:

1. An enrichment agent detects a potential relationship (via embeddings, co-occurrence, shared attributes, or cross-system identity resolution)
2. It proposes a typed edge: "Person X mentioned in Block A and assigned to JIRA-456 — link them?"
3. The user confirms or rejects at System 1 speed (1-2 seconds per decision) in Orient mode
4. Confirmed edges become permanent structure; rejected proposals are discarded

Each confirmed edge increases graph density without adding nodes. Denser graphs produce better future proposals — a compounding flywheel. See [../Vision/AI.md](../Vision/AI.md) §The Integrator for the full interaction design.

**Cross-system entity resolution** is a special case: the same person appears as a Todoist assignee, JIRA reporter, and calendar attendee. The Integrator proposes merges based on matching email, username, or name — the user confirms which are truly the same entity.

### Relationship to Existing Types

Built-in types (`Block`, `Document`) use the compile-time `#[derive(Entity)]` macro which generates `IntoEntity` + `TryFromEntity` + `TypeDefinition`. User-defined types use YAML definitions that produce `TypeDefinition` at runtime. The two coexist:

| Type | Definition | Schema | Extension table |
|---|---|---|---|
| Block | `#[derive(Entity)]` in Rust | Compile-time `IntoEntity`/`TryFromEntity` | `block` table (universal) |
| Document | `#[derive(Entity)]` in Rust | Compile-time `IntoEntity`/`TryFromEntity` | `documents` table |
| Person | YAML in `types/person.yaml` | Runtime from type definition | `person` table (generated) |
| Book | YAML in `types/book.yaml` | Runtime from type definition | `book` table (generated) |

The generated extension tables follow the same conventions as the compile-time tables: same column types, same index patterns, same `id TEXT PRIMARY KEY` contract. The `SchemaModule` trait is the unifying abstraction — both built-in and user-defined types implement it.

### Computed Fields and Prototype Blocks

Computed fields in type definitions subsume the current **prototype block** mechanism (see [../Vision/PetriNet.md](../Vision/PetriNet.md) §WSJF-Based Task Sorting). Prototype blocks define `=`-prefixed Rhai expressions that are topo-sorted and evaluated at materialization time. In the entity type system, these become `lifetime: computed` fields in the type schema:

```yaml
# Before: prototype block with =expressions
# properties:
#   priority_weight: "=switch priority { 3.0 => 100.0, ... }"
#   task_weight: "=priority_weight * (1.0 + urgency_weight)"

# After: computed fields in type definition
name: task
fields:
  priority:        { type: integer, lifetime: persistent }
  deadline:        { type: date, lifetime: persistent }
  priority_weight:
    type: real
    lifetime: computed
    expr: "switch priority { 3.0 => 100.0, 2.0 => 40.0, 1.0 => 15.0, _ => 1.0 }"
  task_weight:
    type: real
    lifetime: computed
    expr: "priority_weight * (1.0 + urgency_weight) + position_weight"
```

The dependency graph between computed fields is visible in one place, the topo-sort operates over the schema's computed fields, and the evaluation context is well-defined. Per-instance overrides still work: if a block's persistent properties contain a literal value for a computed field's key, the literal wins.

The render DSL becomes purely about **presentation** — which columns to show, in what layout — and no longer carries computation logic.

