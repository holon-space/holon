# Schema Reference

*Part of [Architecture](../Architecture.md). Table/view-level reference for
the Turso projection — see [Model](Model.md) for the five-layer mental model
and [Storage](Storage.md) for the Cell/QueryableCache/CDC machinery that
reads and writes these objects.*

## Shape of the projection

There is no `documents` table and no `doc:` / `holon-doc://` entity scheme.
Everything is a block: pages are blocks tagged `Page`, tasks are blocks with
a `block_type`, properties live in a nested JSON column. The only
non-`block:` scheme that appears in `parent_id` is the root sentinel,
`sentinel:no_parent` — root detection is `parent_id LIKE 'sentinel:%'`, full
stop (the legacy `doc:%`-prefix branch was dropped from
`blocks_with_paths.sql`; see the
`crates/holon-turso/sql/schema/blocks_with_paths.sql` history).

Turso is a **projection**, not an authority (Model §Five layers, row 3):
exactly one writer, verbatim, never re-merged, ephemeral by contract. Base
tables hold structural truth; materialized views (matviews) hydrate them for
readers. Turso's incremental view maintenance (IVM) fires CDC only on
matviews, not on base tables — chained matviews (a matview selecting from
another matview) are supported and used deliberately (`block_requirement_edges`
joins the `block` matview, not `block_raw`).

## Module registry

Every table/view is owned by exactly one `SchemaModule` (trait in
`crates/holon-turso/src/schema_module.rs`) — including `graph_eav`, whose
former "wired directly as DI SQL" exception no longer exists
(`GraphEavSchemaModule` is a regular module). `provides()`/`requires()`
resources are FluxDI `DbReady<R>` markers; the concrete module impls live in
`crates/holon-turso/src/schema_modules.rs` and are run via
`run_schema_module` by the `DbReady<R>` providers in
`crates/holon/src/di/schema_providers.rs`. Dependency ordering — e.g.
`block_with_path` after the `block` matview — is resolved by FluxDI's
`resolve_all_eager()`, not by a hand-rolled topological sort. Note the
import path: consumers import `holon_turso::schema_modules` directly (as
`schema_providers.rs` does) — `crates/holon/src/storage/mod.rs` re-exports
the `schema_module` trait module, `dynamic_schema_module`, `turso`,
`resource`, etc., but **not** `schema_modules`, so there is no
`crate::storage::schema_modules` path.

| Module | Provides | Requires | Kind |
|--------|----------|----------|------|
| `CoreSchemaModule` | `block_raw`, `directory`, `file` | (none) | base tables |
| `BlockSchemaModule` | `block_requires`, `block_tags` | `block_raw` | junction tables |
| `BlockMatviewSchemaModule` | `block` | `block_raw`, `block_requires`, `block_tags` | matview |
| `BlockRequirementEdgesSchemaModule` | `block_requirement_edges` | `block`, `block_requires` | chained matview |
| `BlockHierarchySchemaModule` | `block_with_path` | `block` | matview (recursive CTE) |
| `NavigationSchemaModule` | `navigation_history`, `navigation_cursor`, `current_focus`, `focus_roots` | `block` | tables + 2 matviews |
| `SyncStateSchemaModule` | `sync_states` | (none) | base table |
| `OperationsSchemaModule` | `operation` | (none) | base table |
| `LinkSchemaModule` | `block_link` | `block` | base table (populated by `LinkEventSubscriber`, not SQL) |
| `IdentitySchemaModule` | `canonical_entity`, `entity_alias`, `proposal_queue` | (none) | base tables (unpopulated seam) |
| `GraphEavSchemaModule` | `graph_eav` (`nodes`, `edges`, `node_labels`, `property_keys`, `*_props_*`) | (none) | base tables |

**Runtime-defined types**: `DynamicSchemaModule`
(`crates/holon-turso/src/dynamic_schema_module.rs`) builds a `SchemaModule`
from a `TypeDefinition`, creating one extension table per user-defined type
that foreign-keys to `block(id)`. Its live invocation is the MCP
`create_entity_type` tool (`frontends/mcp/src/tools.rs`): at tool-call time
— outside the startup provider graph — it constructs the module, runs
`ensure_schema`, and calls `mark_available(provides())` so the resource
marker still lands. There is also a lower seam,
`StorageBackend::create_entity` (`crates/holon-turso/src/turso.rs`), which
runs `to_create_table_sql` DDL directly with no module ownership and no
resource marker.

## Block: base table, junctions, and hydration matview

`block_raw` is the structural base table. Writes always target it directly.
Reads that need the edge-typed fields go through the `block` matview, which
LEFT-JOINs the two junction tables and folds each into a JSON array with
`json_group_array(...) FILTER (WHERE ... IS NOT NULL)`, defaulting to `'[]'`
when a block has no tags/requires:

| Table/view | Columns | Role |
|---|---|---|
| `block_raw` | `id`, `parent_id`, `depth`, `sort_key`, `content`, `content_type`, `source_language`, `source_name`, `properties`, `marks`, `collapsed`, `completed`, `block_type`, `created_at`, `updated_at`, `_change_origin` | Base table. Owned by `CoreSchemaModule`. All structural writes land here. |
| `block_tags` | `block_id`, `tag` (PK on both, FK `block_id → block_raw(id)` cascade) | Junction table for the `tags` edge field. Owned by `BlockSchemaModule`. |
| `block_requires` | `block_id`, `required_id` (PK on both, FKs to `block_raw(id)` cascade) | Junction table for the `requires` edge field. Owned by `BlockSchemaModule`. |
| `block` (matview) | all `block_raw` columns + `tags` (JSON array), `requires` (JSON array) | Hydrated read surface. Owned by `BlockMatviewSchemaModule`. Every downstream reader (GQL/PRQL, `block_with_path`, `block_requirement_edges`) reads from here, not `block_raw`. |
| `block_requirement_edges` (matview) | `block_id`, `required_id`, `required_content` | `JOIN`s `block_requires` against the `block` matview (chained matview — the join target is itself a matview). Owned by `BlockRequirementEdgesSchemaModule`. |

`block_tags`/`block_requires` are **edge fields**: multi-valued fields that
project to a junction table instead of folding into the `properties` blob.
`BlockSchemaModule::edge_fields()` returns one `EdgeFieldDescriptor` per
field (`entity`, `field`, `join_table`, `source_col`, `target_col`) —
consumed both by the write path (`SqlOperationProvider` routes a
`Value::Array` payload for a matching field through DELETE+INSERT against
`join_table` instead of `properties`) and by the read path
(`graph_schema::build()` wires a `JoinTableEdgeResolver`). They participate
in CDC/change detection like any other column: a tag add/remove is a
`block_tags` row insert/delete, which the `block` matview's CDC propagates
as a changed `block` row.

`DROP TABLE IF EXISTS task_blockers` runs before `block_requires` is
(re)created — `task_blockers` is the pre-rename name for the junction table
and is dropped rather than migrated, since matviews and edge-field
descriptors all reference `block_requires` now.

### Row → `Block` at the boundary

SQL rows never flow into the app as untyped maps. `impl TryFrom<StorageEntity>
for Block` (`crates/holon-api/src/block.rs`) parses each column explicitly —
an absent `parent_id` key is a reader bug (hard error naming the column and
block id), a `NULL` `parent_id` is the legal root case
(`EntityUri::no_parent()`), and a malformed `content_type`/`source_language`
fails loud with the offending value in the message. The wire DTO,
`BlockWire`, converts `Block ↔ BlockWire` at the process boundary (MCP,
serialization) — internal code passes `Block`, never a raw row, past the
`TryFrom` boundary.

## Hierarchy: `block_with_path`

`block_with_path` (`crates/holon-turso/sql/schema/blocks_with_paths.sql`,
owned by
`BlockHierarchySchemaModule`) is a recursive-CTE matview over the `block`
matview (not `block_raw`) — another chained matview. Root detection is
`parent_id LIKE 'sentinel:%'`; the recursive case joins a block to its
parent's row in the CTE and appends `/id` to the accumulated `path`:

| Column | Description |
|---|---|
| `id`, `parent_id`, `content`, `content_type`, `source_language`, `source_name`, `properties`, `created_at`, `updated_at` | Passed through from `block`. |
| `path` | `/` + `/`-joined ancestor chain, e.g. `/root_id/.../id`. |
| `root_id` | The root block's id, carried down through every recursive step. |

Path-prefix matching against this view (`path LIKE '<prefix>%'`) is how
`from descendants` context queries work — see `prql_stdlib.prql`. The
`roots` PRQL stdlib function that used to wrap sentinel-root filtering was
deleted (2026-07-02); callers filter `parent_id LIKE 'sentinel:%'` directly
or query `block_with_path` where `root_id == id`.

## Navigation

Owned by `NavigationSchemaModule` (requires `block`, because the
`focus_roots` matview JOINs the `block` matview; the DI provider accordingly
depends on `DbReady<BlockMatviewView>` — `schema_providers.rs`).
`navigation_history` is an append-only log
of navigation events per region; `closed_at IS NULL` marks the still-open
entries (soft-close, not delete, so back/forward history survives closing a
tab). `navigation_cursor` holds, per region, which history row is "current".
Two matviews derive read-optimized views from these tables:

| Table/view | Columns | Role |
|---|---|---|
| `navigation_history` | `id`, `region`, `block_id`, `timestamp`, `closed_at` | Append-only history. `block_id IS NULL` rows record "navigated home". |
| `navigation_cursor` | `region` (PK), `history_id` (FK) | Current position per region. |
| `current_focus` (matview) | `region`, `block_id`, `timestamp` | `navigation_cursor JOIN navigation_history`: the currently-focused block per region. |
| `focus_roots` (matview) | `region`, `root_id`, `added_ts`, `history_id` | Open (`closed_at IS NULL`), non-home (`block_id IS NOT NULL`) history rows. Consumers `CHILD_OF*0..N` from `root_id` to render focus + descendants; `history_id` is the sidebar's close-button handle. |

The editor caret/cursor is **not** persisted here — that was removed
(`editor_cursor` table + `current_editor_focus` matview deleted, ADR 0010) as
pure in-memory UI state.

## Sync, operations, links, identity

| Table | Columns | Role |
|---|---|---|
| `sync_states` | `provider_name` (PK), `sync_token`, `updated_at`, `_change_origin` | One row per external sync provider (Todoist, etc.); owned by `SyncStateSchemaModule`. |
| `operation` | `id`, `operation`, `inverse`, `status`, `created_at`, `display_name`, `entity_name`, `op_name`, `_change_origin` | Undo/redo log; owned by `OperationsSchemaModule`. Schema must match `OperationLogEntry` in `holon-core/src/operation_log.rs`. Part of the command-sourcing seam kept warm for future offline (see Model.md "Offline (future)"). |
| `block_links` | `source_block_id`, `target`, `kind`, `resolved_id` (PK `source_block_id, target, kind`) | Link junction derived from each block's Link marks, rewritten in the same transaction as the block row. `kind` classifies the target: `page` (wiki name), `block` (block-id URI), `tag`, or `entity` (registered integration scheme, e.g. `cc-session:`). `resolved_id` holds the resolved target id — filled at parse time for `block` and `entity` targets, `NULL` for a dangling page link. Soft targets by design: no foreign keys, because a link may name a page that does not exist yet. The `backlinks` matview keys on `resolved_id` regardless of scheme. |
| `canonical_entity` | `id` (PK), `kind`, `primary_label`, `created_at` | Cross-system entity identity. |
| `entity_alias` | `canonical_id` (FK), `system`, `foreign_id`, `confidence` (PK `system, foreign_id`) | Foreign-system aliases for a canonical entity. |
| `proposal_queue` | `id` (PK), `kind`, `evidence_json`, `status`, `created_at` | Pending merge/identity proposals. |

`canonical_entity`/`entity_alias`/`proposal_queue` are owned by
`IdentitySchemaModule` and are empty by default — the tables exist so future
merge / propose-merge / accept-proposal operations have a schema seam to
plug into rather than growing ad-hoc identity columns elsewhere.

## Generic graph: `graph_eav`

`graph_eav` (`crates/holon-turso/sql/schema/graph_eav.sql`) is owned by
`GraphEavSchemaModule` — a regular `SchemaModule` impl
(`schema_modules.rs`), empty `requires()`, run via `run_schema_module`; only
the `DbReady<GraphEavSchema>` marker remains DI-side
(`di/schema_providers.rs`). It is a generic entity-attribute-value graph
store, independent of the block schema:

| Table | Role |
|---|---|
| `nodes` | Node ids (`id` autoincrement). |
| `edges` | Typed edges between nodes: `source_id`, `target_id`, `type`. |
| `property_keys` | Interned property key strings. |
| `node_labels` | Multi-valued labels per node. |
| `node_props_{int,text,real,bool,json}` | One EAV table per value type, keyed `(node_id, key_id)`. |
| `edge_props_{int,text,real,bool,json}` | Same shape, keyed `(edge_id, key_id)`. |

This is a separate concern from the block hierarchy — it is not currently a
projection of `block`/`block_with_path`.

## Directories and files

`directory` and `file` (owned by `CoreSchemaModule`, alongside `block_raw`)
track the on-disk vault layout independent of block content:

| Table | Columns |
|---|---|
| `directory` | `id` (PK), `name`, `parent_id`, `depth`, `_change_origin` |
| `file` | `id` (PK), `name`, `parent_id`, `content_hash`, `document_id`, `_change_origin` |

## Key files

| Path | Description |
|------|-------------|
| `crates/holon-turso/src/schema_module.rs` | `SchemaModule` trait, `EdgeFieldDescriptor` |
| `crates/holon-turso/src/dynamic_schema_module.rs` | `DynamicSchemaModule` (runtime-defined types) |
| `crates/holon-turso/src/schema_modules.rs` | Concrete built-in module implementations (imported as `holon_turso::schema_modules` directly — not re-exported via `crates/holon/src/storage/mod.rs`) |
| `crates/holon/src/storage/resource.rs` | Re-export of `Resource` (now in `holon-core`) |
| `crates/holon/src/di/schema_providers.rs` | FluxDI `DbReady<R>` wiring, dependency ordering, `graph_eav` DI registration |
| `crates/holon-turso/sql/schema/*.sql` | DDL for every table/matview above |
| `crates/holon/sql/prql_stdlib.prql` | `children`/`siblings`/`descendants`/context-aware PRQL helpers over `block`/`block_with_path` |
| `crates/holon-api/src/block.rs` | `Block`, `BlockWire`, `TryFrom<StorageEntity> for Block` |

See [Storage](Storage.md) for how these tables/views feed the Cell Registry
and CDC pipeline, and [Model](Model.md) for the invariants (verbatim
projection, single writer, sinks never re-merge) that constrain everything
in this file.
