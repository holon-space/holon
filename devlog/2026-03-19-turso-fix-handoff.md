# Turso Bug Fix: IVM matview inconsistency when DML runs inside explicit transactions

## Bug Description

When `INSERT OR REPLACE INTO block` runs inside a `BEGIN TRANSACTION / COMMIT` block, the recursive CTE matview `block_with_path` becomes inconsistent — it retains a stale row that a fresh `SELECT` on the same SQL does not return. The same DML in auto-commit mode works correctly.

This causes CDC callbacks to report `changes=0` for all block-based matviews after subsequent UPDATEs, breaking reactive UI updates in the holon application.

## Reproduction

### Prerequisites

```bash
cd ~/Workspaces/pkm/holon
cargo build --manifest-path tools/Cargo.toml
```

### Run the reproducer

```bash
cargo run --manifest-path tools/Cargo.toml --bin turso-sql-replay -- \
  replay devlog/2026-03-19-turso-ivm-repro-minimal.sql --check-after-each
```

### Expected output

```
VERDICT: No IVM inconsistencies detected.
```

### Actual output

```
!!! [stmt#364] INCONSISTENCY in block_with_path: matview=1 rows, raw=0 rows
=== BUG REPRODUCED at statement 364! ===
  VERDICT: IVM BUG REPRODUCED!
```

The matview `block_with_path` shows a stale `block:root-layout` row that does not exist when the same SQL is re-executed fresh.

## What the reproducer does

The `.sql` replay file contains 364 statements extracted from a live holon session:

1. **Schema creation** (DDL): Creates `block`, `document`, `navigation_*`, `events` tables with indices
2. **Matview chain creation** (DDL):
   - `current_focus` — JOIN of `navigation_cursor` + `navigation_history`
   - `focus_roots` — depends on `current_focus` + `block`
   - `block_with_path` — recursive CTE on `block` (WITH RECURSIVE paths AS ...)
   - Several `watch_view_*` matviews on `block`
3. **Navigation seed** (DML): INSERT into `navigation_cursor` and `navigation_history`
4. **Events insertion** (transactions): INSERT INTO events — works fine
5. **Block insertion** (transaction, stmt 363-364): `BEGIN TRANSACTION` → `INSERT OR REPLACE INTO block` → **BUG TRIGGERS HERE**

The critical difference: steps 1-4 work correctly. Step 5 — the first `INSERT OR REPLACE INTO block` **inside an explicit transaction** — causes `block_with_path` to diverge from its underlying query.

## Key evidence

- **Auto-commit works**: The same replay WITHOUT `BEGIN/COMMIT` (all statements as auto-commit) produces correct CDC events and no matview inconsistency
- **Transactions break it**: Adding `BEGIN TRANSACTION` / `COMMIT` around the block INSERT batch reproduces the bug 100% of the time
- **Matview data vs CDC**: After the inconsistency, subsequent `UPDATE block SET properties = json_set(...)` causes CDC callbacks to fire for ALL matviews but with `changes=0`. The matview data IS queryable with correct values, but the IVM delta pipeline is broken.

## Analysis

### Affected matview

```sql
CREATE MATERIALIZED VIEW block_with_path AS
  WITH RECURSIVE paths AS (
    SELECT id, parent_id, content, content_type, source_language, source_name,
           properties, created_at, updated_at,
           '/' || id AS path, id AS root_id
    FROM block
    WHERE parent_id LIKE 'doc:%' OR parent_id LIKE 'sentinel:%'
    UNION ALL
    SELECT b.id, b.parent_id, b.content, b.content_type, b.source_language,
           b.source_name, b.properties, b.created_at, b.updated_at,
           p.path || '/' || b.id AS path, p.root_id
    FROM block b INNER JOIN paths p ON b.parent_id = p.id
  )
  SELECT * FROM paths
```

### The failing DML

```sql
BEGIN TRANSACTION;
INSERT OR REPLACE INTO block ("updated_at", "created_at", "content", "document_id",
  "parent_id", "content_type", "id", "properties")
VALUES (1773940561981, 1773940561939, 'Holon Layout',
  'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761',
  'doc:be7f2579-8ddb-4f09-b1f4-ceed17a42761',
  'text', 'block:root-layout', '{"ID":"root-layout","sequence":0}');
-- ... more INSERTs ...
COMMIT;
```

### Root cause hypothesis

The DBSP delta computation for recursive CTE matviews doesn't correctly handle the transaction commit path. When `apply_view_deltas` runs at `COMMIT` time (processing all statement deltas accumulated during the transaction), the recursive CTE's fixpoint computation produces an incorrect final delta — retaining a row that should have been removed or not including a deletion.

This is consistent with the earlier finding in `HANDOFF_IVM_UPDATE_CDC.md` where delta ordering (delete before insert) matters for UPDATE propagation. In the transaction case, the accumulated deltas from multiple INSERTs may have ordering issues in the recursive CTE's consolidation step.

### Relevant Turso code locations

- `core/vdbe/execute.rs` — Halt handler auto-commit vs explicit commit paths (line ~2241)
- `core/vdbe/mod.rs` — `apply_view_deltas` (line ~1492)
- `core/incremental/compiler.rs` — `CommitState::UpdateView` and `delta.consolidate()`
- `core/vdbe/execute.rs` — `ApplyViewChange` substage in `op_insert` (line ~8649)

### What to compare

The most direct debugging approach:
1. Run the reproducer with `--check-after-each` and note that stmt 364 is the first inconsistency
2. Add tracing in `apply_view_deltas` to log the delta changes for `block_with_path` at COMMIT time
3. Compare the delta with what auto-commit produces for the same INSERT

## Fix status

### First bug (FIXED in Turso commit 478c8109)
- Reproducer: `devlog/2026-03-19-turso-ivm-repro-minimal.sql` (364 stmts)
- Symptom: `block_with_path` shows 1 EXTRA stale row after first INSERT in transaction
- Status: **FIXED** — reproducer now passes clean

### Second bug (FIXED in Turso commit e437a5b5)
- Reproducer: `devlog/2026-03-19-turso-ivm-repro-v2.sql` (368 stmts)
- Symptom: `block_with_path` has 4 rows but raw query returns 5 — **MISSING** row
- Missing row: `block:block:left_sidebar::render::0` (child of `block:e7fcc60b`, content_type=source, source_language=render)
- Triggers at the 5th `INSERT OR REPLACE INTO block` in the same transaction
- Status: **FIXED** — reproducer now passes clean

### Third bug (OPEN)
- Reproducer: `devlog/2026-03-19-turso-ivm-repro-v3.sql` (471 stmts)
- Symptom: `block_with_path` has **236 rows** but raw query returns **38** — massive divergence (198 extra stale rows)
- Triggers at stmt 466: 2nd INSERT in a **second** `BEGIN TRANSACTION` block (after the first transaction committed at stmt 460)
- The second transaction inserts blocks into a different document than the first transaction
- Between the two transactions, additional matviews are created (`events_view_directory`, `events_view_file`)

```
!!! [stmt#466] INCONSISTENCY in block_with_path: matview=236 rows, raw=38 rows
  EXTRA rows in matview (not in fresh):
    block:default-layout-root | doc:1b2f1c05 | Holon Layout | text
    block:default-layout-root::render::0 | block:default-layout-root | columns(...) | source | render
```

### Pattern across all three bugs

| Bug | Transaction | Symptom | INSERT count in tx |
|-----|------------|---------|-------------------|
| v1 | 1st tx, 1st INSERT | 1 extra stale row | 1 |
| v2 | 1st tx, 5th INSERT | 1 missing row | 5 |
| v3 | 2nd tx, 2nd INSERT | 198 extra stale rows | 2 (but after prior tx + DDL) |

All affect `block_with_path` (recursive CTE matview). The divergence worsens with more data and more transactions. The common pattern: `INSERT OR REPLACE INTO block` inside `BEGIN TRANSACTION / COMMIT` causes the recursive CTE's DBSP fixpoint to produce incorrect deltas.

### How to validate

```bash
# First bug (should pass — FIXED):
cargo run --manifest-path tools/Cargo.toml --bin turso-sql-replay -- \
  replay devlog/2026-03-19-turso-ivm-repro-minimal.sql --check-after-each

# Second bug (should pass — FIXED):
cargo run --manifest-path tools/Cargo.toml --bin turso-sql-replay -- \
  replay devlog/2026-03-19-turso-ivm-repro-v2.sql --check-after-each

# Third bug (should also pass after fix — OPEN):
cargo run --manifest-path tools/Cargo.toml --bin turso-sql-replay -- \
  replay devlog/2026-03-19-turso-ivm-repro-v3.sql --check-after-each

# Full replay (ultimate validation):
cargo run --manifest-path tools/Cargo.toml --bin turso-sql-replay -- \
  replay devlog/2026-03-19-turso-ivm-cdc-replay-tx.sql --check-after-each
```

## Acceptance Criteria

- [x] `devlog/2026-03-19-turso-ivm-repro-minimal.sql` passes (first bug — FIXED)
- [x] `devlog/2026-03-19-turso-ivm-repro-v2.sql` passes (second bug — FIXED)
- [ ] `devlog/2026-03-19-turso-ivm-repro-v3.sql` passes (third bug — OPEN)
- [ ] `devlog/2026-03-19-turso-ivm-cdc-replay-tx.sql` passes (full replay)
- [ ] Existing Turso tests still pass
- [ ] New test covers: recursive CTE matview + multiple explicit transactions + INSERT OR REPLACE
- [ ] Changes are minimal and focused

## Files

| File | Purpose |
|------|---------|
| `devlog/2026-03-19-turso-ivm-repro-minimal.sql` | First reproducer — 364 stmts, FIXED |
| `devlog/2026-03-19-turso-ivm-repro-v2.sql` | Second reproducer — 368 stmts, FIXED |
| `devlog/2026-03-19-turso-ivm-repro-v3.sql` | Third reproducer — 471 stmts, OPEN |
| `devlog/2026-03-19-turso-ivm-cdc-replay-tx.sql` | Full replay with all 1645 statements |
| `tools/src/turso_sql_replay.rs` | Replay tool (handles `BEGIN/COMMIT` boundaries) |

## How to run the reproducer from the Turso repo

The replay tool links against Turso directly. From the holon repo:

```bash
cargo run --manifest-path tools/Cargo.toml --bin turso-sql-replay -- \
  replay devlog/2026-03-19-turso-ivm-repro-minimal.sql --check-after-each
```

## Turso Repo

`~/Workspaces/bigdata/turso/` (branch: `holon`)
