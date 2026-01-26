---
name: holon-live-mcp-debugging
description: |
  Debug the Holon Flutter app's live state using the holon-live MCP server. Use when:
  (1) Flutter app shows blank/empty screen or wrong content, (2) need to inspect what data
  the backend actually has (documents, blocks, navigation state), (3) need to test PRQL queries
  with context parameters (from children, from siblings, from descendants), (4) need to verify
  materialized view correctness vs raw SQL, (5) need to trace the initial_widget() rendering
  pipeline, (6) need to capture SQL traces for Turso IVM bug reproduction.
  The holon-live MCP shares the same database as the running Flutter app.
author: Claude Code
version: 1.1.0
date: 2026-03-04
---

# Debugging Holon Flutter App via holon-live MCP

## Problem

The Holon Flutter app renders a backend-driven UI from database state. When something doesn't
display correctly, you need to inspect the live database to understand what the backend sees.
The holon-live MCP server connects to the same database as the Flutter app, giving you direct
inspection capabilities.

## Context / Trigger Conditions

- Flutter app shows blank screen, missing panels, or garbled layout
- A specific document (e.g., index.org) doesn't render its content
- PRQL queries return unexpected results
- Navigation doesn't work (clicking documents does nothing)
- Layout columns appear but with wrong sizes or missing content

## Debugging Playbook

### Step 1: Check the Data Model

The database has three key tables. Always start by understanding what's loaded:

```sql
-- Documents hierarchy (index.org is the root container)
SELECT id, name, parent_id FROM documents;

-- Block count and structure
SELECT COUNT(*) FROM blocks;
SELECT id, parent_id, content, content_type, source_language
FROM blocks ORDER BY parent_id, sort_key LIMIT 30;
```

Key relationships:
- `documents.id = "index.org"` is the root container (parent_id="__no_parent__")
- Child documents have `parent_id = "index.org"`
- Blocks reference documents via `parent_id = "holon-doc://{document_uuid}"`
- The `holon-app-layout` block is the root layout block

### Step 2: Verify the Root Layout Block

The Flutter app calls `initial_widget()` which finds the root layout block via this query:

```sql
SELECT b.id, b.parent_id, b.content, b.properties, src.content as prql_source
FROM blocks b
INNER JOIN blocks src ON src.parent_id = b.id
    AND src.content_type = 'source'
    AND src.source_language = 'prql'
INNER JOIN documents d ON b.parent_id = 'holon-doc://' || d.id
WHERE d.name = 'index' AND d.parent_id = 'index.org'
ORDER BY b.id LIMIT 1;
```

If this returns empty, the app can't render anything. Check:
- Does a document with `name = 'index'` and `parent_id = 'index.org'` exist?
- Does it have blocks with `parent_id = 'holon-doc://{that_document_id}'`?
- Do those blocks have children with `content_type = 'source'` and `source_language = 'prql'`?

### Step 3: Test PRQL Queries with Context

Virtual tables (`from children`, `from siblings`, `from descendants`) require context parameters.
Always pass them via the MCP `execute_prql` tool:

```json
{
  "prql": "from children\nfilter content_type != \"source\"\nrender (list sortkey:id item_template:(text this.content))",
  "context_id": "holon-app-layout",
  "context_parent_id": "holon-doc://f3a17d22-..."
}
```

If `from children` returns empty but `from blocks filter parent_id == "X"` returns rows,
the context parameters aren't being passed correctly.

### Step 4: Check Navigation State

Navigation drives what the main panel displays:

```sql
-- Current cursor (should have non-null history_id for active regions)
SELECT * FROM navigation_cursor;

-- Navigation history entries
SELECT * FROM navigation_history ORDER BY id DESC LIMIT 10;

-- What's actually focused (JOIN of cursor + history)
SELECT * FROM current_focus;
```

If `current_focus` is empty, no document is navigated to. The left sidebar's document
list click handler triggers `navigation.focus` to populate this.

### Step 5: Verify Materialized Views

Materialized views are the live data source for Flutter. Compare matview output to raw SQL:

```sql
-- List all matviews
SELECT name, sql FROM sqlite_master WHERE type='view' AND name LIKE 'watch_view%';

-- Check matview schema (should match the SELECT columns)
PRAGMA table_info(watch_view_XXXX);

-- Compare matview data vs raw query
SELECT * FROM watch_view_XXXX;
-- vs running the matview's SQL directly as a regular query
```

**Known Turso IVM bug**: `SELECT *, derived_expr AS alias` in materialized views drops the
`*` columns and misaligns derived column values. If a matview has fewer columns than expected
or wrong values, this is likely the cause. Workaround: explicitly list all columns.

### Step 6: Check Source Blocks

Each panel (main, left_sidebar, right_sidebar) has a `::src::0` child block containing PRQL:

```sql
SELECT id, parent_id, content FROM blocks
WHERE content_type = 'source' AND source_language = 'prql';
```

You can test any of these PRQL queries via `execute_prql` with appropriate context.

### Step 7: Capture SQL Trace for IVM Bug Reproduction

When matviews are corrupted (stale rows, negative weights, missing data), capture a full SQL
trace to create a reproducer for the Turso team.

**Launch with tracing:**
```bash
# Delete old DB for clean capture
rm -f ~/Library/Application\ Support/space.holon/holon.db*

# Launch with full SQL + param tracing
cd frontends/flutter
HOLON_TRACE_SQL=1 RUST_LOG="holon::storage::turso=trace" flutter run -d macos 2>&1 | tee /tmp/flutter-sql-trace.log
```

**Extract replay SQL with `scripts/extract-sql-trace.py`:**
```bash
# Navigation-only (focus_roots / current_focus bugs)
python3 scripts/extract-sql-trace.py /tmp/flutter-sql-trace.log \
  --include navigation_cursor,navigation_history,current_focus,focus_roots

# All tables except noisy task/session sync
python3 scripts/extract-sql-trace.py /tmp/flutter-sql-trace.log \
  --exclude task,session

# Time-windowed (around when the bug manifested)
python3 scripts/extract-sql-trace.py /tmp/flutter-sql-trace.log \
  --after "2026-03-04T10:13:00" --before "2026-03-04T10:14:00"
```

The script:
- Inlines named `$param` and positional `?` parameters into the SQL
- Adds `-- Wait Nms` timing comments between statements
- Deduplicates DDL (keeps `actor_ddl`, skips outer `execute_ddl`)
- Filters by table name (matches SQL template, not param values)

**Detecting corruption during a session:**
```sql
-- Check if current_focus matches navigation_cursor
SELECT nc.history_id, nh.block_id AS cursor_doc, cf.block_id AS matview_doc
FROM navigation_cursor nc
LEFT JOIN navigation_history nh ON nc.history_id = nh.id
LEFT JOIN current_focus cf ON cf.region = nc.region
WHERE nc.region = 'main';
-- If cursor_doc != matview_doc, the matview is stale

-- Check focus_roots consistency
SELECT COUNT(*) as matview_cnt FROM focus_roots WHERE region = 'main';
-- Compare to raw SQL re-evaluation of the same query
```

## Common Issues

| Symptom | Likely Cause | Check |
|---------|-------------|-------|
| Completely blank screen | `initial_widget()` failed | Root layout block query (Step 2) |
| Layout renders but panels empty | `from children` returns nothing | Context params (Step 3) |
| Sidebar shows no documents | No documents synced | `SELECT * FROM documents` |
| Document click does nothing | Navigation not working | Navigation state (Step 4) |
| Layout columns wrong size | Matview data corrupted | Compare matview vs raw SQL (Step 5) |
| Data visible in SQL but not in UI | Matview stale or buggy | Drop and recreate matview |

## Notes

- The holon-live MCP and Flutter app share the **same database** — the Flutter app launches
  the MCP server alongside itself
- Materialized views are identified by a hash of their SQL — changing query params creates
  a new view
- `sync_states` table tracks what has been synced from external sources (org files, Todoist)
- The `blocks_with_paths` matview provides hierarchical paths for `from descendants` queries
- `_change_origin` column on blocks tracks whether changes came from local edits or remote sync
