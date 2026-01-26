---
name: turso
description: Embedded SQLite fork with IVM, MVCC, CDC, and vector search powering holon's storage
type: reference
source_type: url
source_id: https://github.com/nightscape/turso (branch: holon)
fetch_timestamp: 2026-04-23
---

## Turso (nightscape fork, branch: holon)

**Purpose**: In-process SQLite-compatible database extended with MVCC, Incremental View Maintenance (IVM), CDC, vector search, and FTS. Holon's primary storage backend.

### Key Features Used by Holon

| Feature | Purpose |
|---------|---------|
| IVM (Incremental View Maintenance) | DBSP-powered materialized views; auto-update on data change |
| CDC (Change Data Capture) | Real-time tracking of row mutations → reactive query subscriptions |
| `BEGIN CONCURRENT` | MVCC write concurrency |
| Virtual Tables | Custom FDW (Foreign Data Wrapper) layer for lazy MCP data loading |
| Full-Text Search | Tantivy-based indexing |
| Vector Search | Exact + approximate vector indexing |

### IVM Integration Notes (critical)

- IVM views must be preloaded (`preload_blocks.prql`) **before** file-watch/sync starts to avoid "database is locked" contention
- PRQL queries compile to IVM-backed materialized views
- `navigation_cursor` → `navigation_history` → `current_focus` are IVM materialized views
- Turso IVM join panic and chained matview hang are known issues with workarounds (see `/skills/turso-fix`)

### Schema Notes

- `blocks` table: `created_at TEXT` in DDL but `Block.created_at: i64` in struct → always provide explicit integer millis
- Holon uses custom `turso_sdk_kit` and `turso_ext` for extensions
- `turso_core` with `json` feature for JSON column support

### Keywords
turso, sqlite, IVM, CDC, materialized-view, PRQL, storage, MVCC, virtual-table
