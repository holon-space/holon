---
title: CDC and Streaming (Change Data Capture)
type: concept
tags: [cdc, streaming, reactive, turso, ivm]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - crates/holon/src/storage/turso.rs
  - crates/holon-api/src/streaming.rs
  - crates/holon/src/api/ui_watcher.rs
---

# CDC and Streaming

The data flow from storage mutations to the UI is entirely push-based. The UI never polls.

## Pipeline

```
Database Write
    → Turso CDC Callback
    → coalesce_row_changes()
    → broadcast::Sender<BatchWithMetadata<RowChange>>
    → RowChangeStream (per subscriber)
    → UI handler (ReactiveShell / ReactiveView)
```

## Turso IVM (Incremental View Maintenance)

Turso supports materialized views with IVM — changes to base tables automatically update views and fire CDC callbacks. This is the mechanism behind live queries.

`subscribe_sql(sql)` in `BackendEngine` registers a SQL query as a materialized view and returns its CDC stream. The view fires when any row matching the query changes.

## RowChange

```rust
pub struct RowChange {
    pub relation_name: String,  // table/matview name
    pub change: ChangeData,
}

pub type ChangeData = Change<StorageEntity>;
```

## CDC Coalescing

`coalesce_row_changes()` in `crates/holon/src/storage/turso.rs` optimizes CDC batches:

| Input Pattern | Output | Why |
|---------------|--------|-----|
| `DELETE` + `INSERT` same entity ID | `UPDATE` | IVM represents updates as DELETE+INSERT; prevents widget destroy/recreate |
| `INSERT` + `DELETE` same entity ID | *(dropped)* | Net no-op |
| Standalone INSERT/UPDATE/DELETE | Pass through | No optimization needed |

This is critical — without coalescing, every materialized view update would destroy and recreate UI widgets, causing flicker.

## RowChangeStream

A `tokio_stream::Stream<BatchWithMetadata<RowChange>>`. Each item is a batch of changes from a single transaction.

`BatchWithMetadata` carries:
- `inner: Vec<RowChange>` — the changes
- `metadata: BatchMetadata` — relation name, trace context

## UiEvent

`crates/holon-api/src/streaming.rs` — the higher-level event type sent to frontends:

```rust
pub enum UiEvent {
    Structure { widget_spec: WidgetSpec, generation: u64 },
    Data { batch: BatchMapChangeWithMetadata, generation: u64 },
}
```

- `Structure` fires when the block's structure changes (new `RenderExpr`, children added/removed). The widget tree must be rebuilt.
- `Data` fires when rows in the query result change. Applied as diffs to existing row map.
- `generation` is a monotonic counter. Frontends MUST discard Data events whose generation is older than the last received Structure event.

## BatchMapChange

```rust
pub struct BatchMapChange {
    pub items: Vec<MapChange<String, StorageEntity>>,
}

pub enum MapChange<K, V> {
    Insert { key: K, value: V },
    Update { key: K, old: V, new: V },
    Remove { key: K },
}
```

`BatchMapChangeWithMetadata` wraps this with `BatchMetadata { relation_name, trace_context }`.

## Change Origin Tracking

Every `Change<T>` carries `ChangeOrigin`:

```rust
pub enum ChangeOrigin {
    Local { operation_id: Option<String>, trace_id: Option<String> },
    Remote { operation_id: Option<String>, trace_id: Option<String> },
}
```

Stored in `_change_origin` column so it travels with the data through the entire pipeline (solves async context propagation). Used for:
1. Echo suppression in P2P sync (ignore changes we originated)
2. Distributed tracing (correlate with OpenTelemetry spans)
3. UI attribution (which operation caused this change?)

## UI Keying Requirements

**CRITICAL**: The CDC `id` field in `Updated/Deleted` is the SQLite ROWID, which is reused after DELETE. Widgets MUST key by the entity's own `id` field from `data.get("id")`, not the ROWID.

```rust
// CORRECT
let entity_id = data.get("id").unwrap();
// WRONG - ROWID can be reused
let rowid = change_id;
```

## WatchHandle

`crates/holon-api/src/streaming.rs`:
```rust
pub struct WatchHandle {
    output: mpsc::Receiver<UiEvent>,
    command_tx: mpsc::Sender<WatcherCommand>,
}
```

`WatcherCommand::SetVariant(name)` switches the active entity profile variant without triggering a full structural re-render.

## StreamPosition

```rust
pub enum StreamPosition {
    Beginning,       // receive all current rows as Created events, then new changes
    Version(Vec<u8>), // receive only changes after this opaque version token
}
```

## Related Pages

- [[entities/holon-crate]] — `TursoBackend`, `BackendEngine`, `watch_ui`
- [[concepts/reactive-view]] — how frontends consume these streams
- [[entities/holon-api]] — `Change`, `UiEvent`, `BatchMapChange` type definitions
