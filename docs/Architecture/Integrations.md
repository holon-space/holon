# External Integrations & Frontend

*Part of [Architecture](../Architecture.md)*

## External System Integration

### Integration Pattern

Each external system provides:

1. **SyncProvider** - Fetches data from external API
2. **DataSource** - Read access to cached data
3. **OperationProvider** - Routes operations to external API

```rust
// Todoist example
TodoistSyncProvider
  → Incremental sync with sync tokens
  → HTTP requests to Todoist REST API

TodoistTaskDataSource
  → Implements DataSource<TodoistTask>
  → Reads from QueryableCache

TodoistOperationProvider
  → Routes set_field() to Todoist API
  → Returns inverse operation for undo
```

### Adding a New External System

1. Define entity types implementing `IntoEntity` + `TryFromEntity`
2. Implement `DataSource<T>` for read access
3. Implement domain traits (`TaskOperations`, etc.)
4. Create `SyncProvider` for data synchronization
5. Register in DI container

### MCP Client Integration (holon-mcp-client)

External systems that expose an MCP server can be integrated without writing Rust code per operation. `holon-mcp-client` connects to any MCP server over Streamable HTTP, reads its tool schemas at runtime, and converts them into `OperationDescriptor`s that plug into Holon's existing `OperationDispatcher`.

**Location**: `crates/holon-mcp-client/`

#### Architecture

```
MCP Server (e.g. ai.todoist.net/mcp)
       │
       │  list_tools() → JSON Schema per tool
       ▼
┌─────────────────────────────┐     ┌──────────────────────────┐
│  McpOperationProvider       │◄────│  YAML Sidecar            │
│  • descriptors (cached)     │     │  • entity mapping        │
│  • tool_name_map            │     │  • affected_fields       │
│  • peer (rmcp connection)   │     │  • triggered_by          │
│  • _connection (keep-alive) │     │  • preconditions (Rhai)  │
└──────────┬──────────────────┘     │  • param_overrides       │
           │                        └──────────────────────────┘
           │  implements OperationProvider
           ▼
    OperationDispatcher (aggregates all providers)
```

#### Components

| Component | File | Purpose |
|-----------|------|---------|
| `McpOperationProvider` | `mcp_provider.rs` | Connects to MCP server, caches `OperationDescriptor`s from tool schemas, executes tools via `call_tool`. Holds `McpRunningService` to keep the connection alive. |
| `McpSidecar` | `mcp_sidecar.rs` | YAML config that patches UI affordances onto MCP tools: entity mapping, `affected_fields`, `triggered_by`, `precondition` (Rhai), `param_overrides`. |
| `RhaiPrecondition` | `mcp_sidecar.rs` | Parse-don't-validate wrapper: Rhai expressions are validated at YAML deserialization time. Invalid syntax fails immediately, not at operation execution. |
| `mcp_schema_mapping` | `mcp_schema_mapping.rs` | Converts JSON Schema types to `TypeHint` (String, Bool, Number, OneOf, EntityId via overrides). Walks `inputSchema.properties` to build `Vec<OperationParam>`. |
| `connect_mcp()` | `mcp_provider.rs` | Establishes Streamable HTTP connection to an MCP server, returns `Peer<RoleClient>` + `McpRunningService`. |

#### YAML Sidecar

MCP tool schemas carry parameter types and descriptions but lack UI-specific metadata. The YAML sidecar fills this gap:

```yaml
entities:
  todoist_tasks:
    short_name: task
    id_column: id
  todoist_projects:
    short_name: project
    id_column: id

tools:
  complete-tasks:
    entity: todoist_tasks
    affected_fields: [completed]
    triggered_by:
      - from: completed
        provides: [ids]
    precondition: "completed == false"  # validated as Rhai at load time
  update-tasks:
    entity: todoist_tasks
    affected_fields: [content, description, priority, dueString, labels]
  add-tasks:
    entity: todoist_tasks
    display_name: Create Task
```

Tools without sidecar entries still appear as operations, but with no gesture bindings (affected_fields, triggered_by, preconditions).

#### Tool Name Normalization

MCP tools use kebab-case (`complete-tasks`), Holon operations use snake_case (`complete_tasks`). `McpOperationProvider` maintains a `tool_name_map` to translate between the two.

#### DI Registration (Todoist Example)

`McpOperationProvider` coexists with existing hand-written providers. In `holon-todoist/src/di.rs`:

```rust
// Existing providers (unchanged):
// - TodoistSyncProvider → dyn SyncableProvider + dyn OperationProvider ("todoist.sync")
// - TodoistTaskOperations → dyn OperationProvider (set_field, indent, move_block, etc.)
// - TodoistProjectDataSource → dyn OperationProvider (move_block for projects)

// New MCP provider (additive):
// - McpOperationProvider → dyn OperationProvider (complete_tasks, update_tasks, add_tasks, ...)
//   Wrapped with OperationWrapper for automatic post-operation sync
```

The `TodoistConfig.mcp_server_uri` field controls whether the MCP provider is registered. When set, `McpOperationProvider::connect()` runs inside a `block_on` in the DI factory (safe because factories execute on the main tokio runtime). The sidecar YAML is bundled at compile time via `include_str!`.

#### Reuse Across Integrations

`holon-mcp-client` is integration-agnostic. To add MCP-backed operations for a new system:

1. Create a YAML sidecar with entity mappings and tool annotations
2. Register `McpOperationProvider` in your integration's DI module with the appropriate MCP server URI
3. Optionally wrap with `OperationWrapper` for post-operation sync

## Frontend Architecture

### Flutter FFI Bridge

The Rust backend exposes a minimal FFI surface via `flutter_rust_bridge`:

```rust
// IoC: frontend discovers what to render, backend resolves everything
fn get_root_block_id() -> Result<String>;
fn render_entity(block_id: String, preferred_variant: Option<String>, is_root: bool)
    -> Result<WatchHandle>;  // returns a Stream<UiEvent>

// Operations: frontend dispatches user actions
fn execute_operation(entity: String, op: String, params: HashMap<String, Value>)
    -> Result<Option<String>>;
```

The frontend never sends queries — it only sends block IDs and receives render instructions.

### Reactive Updates

Frontends subscribe to change streams:

```dart
watchChanges().listen((changes) {
  for (change in changes) {
    updateWidget(change.id, change.data);
  }
});
```

No explicit refresh calls—UI state derives from the change stream.

## Dependency Injection

Using `ferrous-di` for service composition:

```rust
pub async fn create_backend_engine<F>(
    db_path: PathBuf,
    setup_fn: F,
) -> Result<Arc<BackendEngine>>

// Registers:
// - TursoBackend
// - OperationDispatcher
// - TransformPipeline
// - Provider modules (Todoist, OrgMode, etc.)
```

