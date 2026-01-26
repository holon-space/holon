# Turso IVM Bug Investigation Handoff

## Status
Two Turso bugs found. Bug 1 (btree panic) is FIXED by `cargo update`. Bug 2 (IVM data mismatch) is reproduced and under minimization.

## Bug 1: Btree Panic (FIXED)
- **Symptom**: GPUI shows blank screen — only gray title bar
- **Root cause**: Turso btree cursor panic `[PageStack::current] current_page=-1` during recursive CTE matview creation
- **Fix**: `cargo update turso turso_core turso_sdk_kit` (691e2ec3 → 2dc3885f)
- **Minimal reproducer**: `/tmp/holon-gpui-replay-minimal.sql` (56 statements)
- Verified: no longer panics with updated Turso

## Bug 2: IVM Recursive CTE Stale Rows (MINIMIZED)
- **Symptom**: `block_with_path` materialized view accumulates phantom rows (87 vs 84 correct in minimal reproducer)
- **Root cause**: Turso IVM doesn't properly cascade-delete derived rows in recursive CTE matviews when upstream rows are updated via INSERT OR REPLACE
- **Minimal reproducer**: `devlog/turso-ivm-rcte-bug-minimal.sql` (89 statements — reduced from 1135 via ddmin)

### Key Files
- **Minimal reproducer**: `devlog/turso-ivm-rcte-bug-minimal.sql` (89 stmts: 1 CREATE TABLE + 1 CREATE MATERIALIZED VIEW + 43 INSERTs + 44 INSERT OR REPLACEs)
- **Full SQL trace**: `/tmp/holon-ivm-bug.sql` (1068 statements)
- **Replay tool**: `crates/holon/examples/turso_sql_replay.rs`
- **Extract script**: `scripts/extract-sql-trace.py`

### How to Reproduce
```bash
# Minimal reproducer (89 statements):
cargo run --example turso_sql_replay -- devlog/turso-ivm-rcte-bug-minimal.sql
# Shows: INCONSISTENCY in block_with_path: matview=87 rows, raw=84 rows
# VERDICT: IVM BUG REPRODUCED!
```

### What the Stale Rows Look Like
3 phantom rows — all children of `block:3fd58f88` ("Cross-Device Sync"):
1. `block:43f329da` — "CollaborativeDoc with ALPN routing"
2. `block:7aef40b2` — "Offline-first with background sync"
3. `block:e148d7b7` — "Iroh P2P transport for Loro documents"

The bug triggers on stmt 89 — an INSERT OR REPLACE on an *unrelated* subtree (`block:1bbec456`, child of `block:b489c622` "Query & Render Pipeline"). The phantom rows belong to a sibling subtree.

### Pattern
1. 43 blocks are inserted into a tree (3-4 levels deep, rooted at `doc:` prefix parents)
2. A recursive CTE matview tracks the full tree with path computation
3. All 44 blocks are re-inserted via INSERT OR REPLACE (same data, simulating org file re-sync)
4. After the last INSERT OR REPLACE, the matview has 3 extra stale rows that don't exist in a freshly-created equivalent matview

The `block_with_path` matview:
```sql
WITH RECURSIVE paths AS (
    SELECT id, parent_id, ... FROM block WHERE parent_id LIKE 'doc:%' OR parent_id LIKE 'sentinel:%'
    UNION ALL
    SELECT b.id, ... FROM block b INNER JOIN paths p ON b.parent_id = p.id
)
SELECT * FROM paths;
```

### Next Steps
1. **File upstream bug** in the Turso fork with the minimal reproducer
2. **Fix in Turso**: The IVM update path for recursive CTEs needs to cascade-delete derived rows when an intermediate row in the recursion chain is updated via INSERT OR REPLACE

### Turso Fork
- Repo: `https://github.com/nightscape/turso.git`, branch `holon`
- Current commit: `2dc3885f` (fixed btree panic, still has IVM mismatch)
