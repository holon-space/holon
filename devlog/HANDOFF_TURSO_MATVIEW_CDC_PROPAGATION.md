# Handoff: Turso IVM — Matview-on-Matview (Chained Matviews)

## Problem

Creating a materialized view (matview B) that JOINs another materialized view (matview A) **hangs indefinitely** in the CLI path. The `CREATE MATERIALIZED VIEW` statement for matview B never returns.

### Concrete example

```sql
-- Base tables
CREATE TABLE navigation_history (id INTEGER PRIMARY KEY, region TEXT, block_id TEXT, timestamp TEXT);
CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER REFERENCES navigation_history(id));

-- Matview A: joins two base tables — works fine
CREATE MATERIALIZED VIEW current_focus AS
SELECT nc.region, nh.block_id, nh.timestamp
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;

-- Matview B: joins a base table with matview A — HANGS HERE
CREATE MATERIALIZED VIEW focused_blocks AS
SELECT b.id, b.parent_id, b.content
FROM blocks b
JOIN current_focus cf ON b.parent_id = cf.block_id
WHERE cf.region = 'main';
```

### Observed behavior (2025-02-24)

| Path | Behavior |
|------|----------|
| **CLI** (`tursodb -q` or piped stdin) | `CREATE MATERIALIZED VIEW` for matview B **hangs indefinitely** |
| **Rust library** (via test runner) | Both creation and CDC propagation **work correctly** |

The Rust library backend passes all chained matview tests including CDC update propagation. The hang is CLI-specific.

### Test coverage

`testing/runner/tests/ivm-chained-matview.sqltest` covers:
- Single matview creation (baseline)
- Matview-on-matview creation
- CDC propagation through chained matviews
- CDC update propagation (change base data, verify chained matview updates)

All tests pass on the Rust backend. CLI backend cannot currently run them (stale flag set in test runner).

## Current workaround

Join through the base tables directly, bypassing the matview:

```sql
-- This works in both CLI and library paths
SELECT b.id, b.parent_id, b.content
FROM blocks b
JOIN navigation_history nh ON b.parent_id = nh.block_id
JOIN navigation_cursor nc ON nh.id = nc.history_id
WHERE nc.region = 'main';
```

This is functionally equivalent but more verbose and prevents composing matviews as reusable abstractions.

## Root cause hypothesis

The CLI's I/O loop handles `CREATE MATERIALIZED VIEW` differently from the library API. The DBSP graph population step likely blocks on async I/O completion that never arrives in the CLI's execution model. The library API's async runtime completes the same operation without issue.

## Files in holon that are affected

- `crates/holon/src/storage/schema_modules.rs:284-292` — `current_focus` matview definition
- `crates/holon/src/api/backend_engine.rs:101-107` — `focused_children` PRQL stdlib (currently uses workaround)
