---
title: holon-api crate (shared types)
type: entity
tags: [crate, types, api, ffi]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon-api/src/lib.rs
  - crates/holon-api/src/block.rs
  - crates/holon-api/src/entity.rs
  - crates/holon-api/src/entity_uri.rs
  - crates/holon-api/src/render_types.rs
  - crates/holon-api/src/streaming.rs
  - crates/holon-api/src/types.rs
  - crates/holon-api/src/reactive.rs
---

# holon-api crate

The **shared type library** for all crates and frontends. Has no frontend-specific dependencies and is safe to use from FFI (flutter_rust_bridge compatible). Everything that crosses a crate boundary goes through types defined here.

## Key Types

### Value

`crates/holon-api/src/lib.rs` — the universal dynamic value type.

```rust
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    DateTime(String),  // RFC3339
    Json(String),      // opaque JSON blob
    Array(Vec<Value>),
    Object(HashMap<String, Value>),
    Null,
}
```

Conversion impls cover `From<i64>`, `From<f64>`, `From<String>`, `From<bool>`, `From<Option<T>>`, `From<Vec<T>>`, `TryFrom<Value> for i64/f64/String/bool`. Also bidirectional `From<serde_json::Value>`.

Key accessors: `as_i64()`, `as_f64()`, `as_bool()`, `as_string()`, `as_datetime()`, `as_array()`, `as_object()`, `is_null()`, `to_display_string()`.

### EntityUri

`crates/holon-api/src/entity_uri.rs` — typed URI for all entities.

Schemes: `block:`, `file:`, `dir:`. Constructors: `EntityUri::block(id)`, `EntityUri::from_raw(uri_str)`. Stores bare ID separately from scheme. Org files store bare IDs; the parser adds schemes at the boundary.

Important: source block IDs like `j-09-::src::0` are valid RFC 3986 schemes. Always use `EntityUri::block()` explicitly instead of `EntityUri::from_raw()` for source blocks.

### Block

`crates/holon-api/src/block.rs` — the core data model unit.

```rust
pub struct Block {
    pub id: EntityUri,
    pub content: String,
    pub content_type: ContentType,
    pub source_language: Option<SourceLanguage>,
    pub source_name: Option<String>,
    pub parent_id: EntityUri,
    pub sort_key: String,
    pub depth: i64,
    pub task_state: Option<TaskState>,
    pub priority: Option<Priority>,
    pub tags: Option<Tags>,
    pub scheduled: Option<Timestamp>,
    pub deadline: Option<Timestamp>,
    pub properties: HashMap<String, Value>,
    pub document_id: EntityUri,
    pub created_at: i64,  // millis
    pub updated_at: i64,
}
```

`BlockContent` enum: `Text { raw }` or `Source(SourceBlock)`. `SourceBlock` carries `language`, `source`, `name`, `header_args`, `results`.

### Typed Domain Types

`crates/holon-api/src/types.rs`:
- `ContentType` — `Headline | Text | Source | Render | HolonPrql | ...`
- `SourceLanguage` — `Rust | Python | Sql | Prql | Rhai | Render | ...`
- `TaskState` — `Todo | InProgress | Done | Cancelled | ...`
- `Priority` — 0–4 integer wrapper
- `Tags` — newtype over `Vec<String>`
- `Region` — `LeftSidebar | MainPanel | RightSidebar`
- `QueryLanguage` — `Prql | Sql | Gql`

### RenderExpr

`crates/holon-api/src/render_types.rs` — the Rhai-based render expression tree.

```rust
pub enum RenderExpr {
    FunctionCall { name: String, args: Vec<Arg> },
    ColumnRef(String),
    Literal(Value),
    BinaryOp { op: BinaryOperator, left: Box<RenderExpr>, right: Box<RenderExpr> },
    Variable(String),
}
```

`Arg` is either positional or named (keyword). `RenderExpr::to_rhai()` serializes back to DSL string. `visible_columns()` recursively collects `ColumnRef` names.

Key functions: `extract_widget_names()`, `to_rhai()`, `visible_columns()`.

### UiEvent

`crates/holon-api/src/streaming.rs` — the tagged enum for reactive UI events.

```rust
pub enum UiEvent {
    Structure {
        widget_spec: WidgetSpec,
        generation: u64,
    },
    Data {
        batch: BatchMapChangeWithMetadata,
        generation: u64,
    },
}
```

- `Structure` fires when the block's render expression or children change
- `Data` fires when rows in the data query change (CDC from Turso IVM)
- `generation` acts as a version fence — frontends discard stale data events

### Change & BatchMapChange

```rust
pub enum Change<T> {
    Created { data: T, origin: ChangeOrigin },
    Updated { id: String, data: T, origin: ChangeOrigin },
    Deleted { id: String, origin: ChangeOrigin },
}

pub struct BatchMapChangeWithMetadata {
    pub inner: BatchMapChange,
    pub metadata: BatchMetadata,
}
```

`ChangeOrigin::Local/Remote` carries `operation_id` and `trace_id` for distributed tracing and echo suppression.

### WatchHandle

```rust
pub struct WatchHandle {
    output: mpsc::Receiver<UiEvent>,
    command_tx: mpsc::Sender<WatcherCommand>,
}
```

`WatcherCommand::SetVariant(name)` switches the active entity profile variant without re-rendering structural changes.

### Reactive Stream Operators

`crates/holon-api/src/reactive.rs`:
- `CdcAccumulator` — turns `Change<T>` streams into keyed map state
- `materialize_map()` — applies `MapDiff` to `HashMap`
- `coalesce()` — merges duplicate change events
- `apply_map_diff()` — applies `MapDiff` mutations
- `ReactiveStreamExt` — extension trait adding `switch_map`, `merge` on streams

### OperationDescriptor

`crates/holon-api/src/render_types.rs`:
```rust
pub struct OperationDescriptor {
    pub name: String,
    pub description: String,
    pub params: Vec<OperationParam>,
    pub affected_fields: Vec<String>,
}
```

`OperationWiring` pairs an `OperationDescriptor` with the entity/field wiring for dispatch. `to_default_wiring()` builds wiring from descriptor defaults.

### ApiError

Structured error type for FFI:
```rust
pub enum ApiError {
    BlockNotFound { id },
    DocumentNotFound { doc_id },
    CyclicMove { id, target_parent },
    InvalidOperation { message },
    NetworkError { message },
    InternalError { message },
}
```

## Related Pages

- [[entities/holon-crate]] — consumes and implements these types
- [[concepts/value-type]] — design rationale for `Value`
- [[concepts/cdc-and-streaming]] — `Change`, `UiEvent`, `BatchMapChange`
- [[concepts/reactive-view]] — `ReactiveStreamExt` usage
- [[entities/holon-frontend]] — interprets `RenderExpr`
