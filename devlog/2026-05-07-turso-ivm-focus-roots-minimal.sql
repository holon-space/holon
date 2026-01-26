-- Turso IVM bug: redundant UPDATE on block_raw drops the row from the
-- hydrating `block` matview (LEFT OUTER JOIN block_tags / task_blockers
-- + json_group_array(...) FILTER + GROUP BY).
--
-- Sequence:
--   stmt#5  INSERT  block_raw row, content='Dple6 lJaGjrHy3 4b'  →  matview ✓
--   stmt#6  UPDATE  content='D' (real change)                     →  matview ✓
--   stmt#7  UPDATE  content='D' (no value change!)                →  matview ✗  <-- BUG
--
-- Run with:
--   turso-sql-replay replay /tmp/trace_v5_minimal.sql --check-after-each --no-break-on-inconsistency
-- Replay reports `INCONSISTENCY in block: matview=0, fresh=1, missing=1`
-- after stmt#7 and the row is gone. row_raw still has the row.
--
-- Pinned Turso revision when reproduced: nightscape@holon 7cf0a2e68a3a
--
-- Minimized replay (7 statements)

-- [actor_ddl]
CREATE TABLE IF NOT EXISTS block_raw (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    depth INTEGER NOT NULL DEFAULT 0,
    sort_key TEXT NOT NULL DEFAULT 'A0',
    content TEXT NOT NULL DEFAULT '',
    content_type TEXT NOT NULL DEFAULT 'text',
    source_language TEXT,
    source_name TEXT,
    properties TEXT,
    marks TEXT,
    collapsed INTEGER NOT NULL DEFAULT 0,
    completed INTEGER NOT NULL DEFAULT 0,
    block_type TEXT NOT NULL DEFAULT 'text',
    created_at INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    _change_origin TEXT
);

-- [actor_ddl]
CREATE TABLE IF NOT EXISTS task_blockers (
    blocked_id TEXT NOT NULL,
    blocker_id TEXT NOT NULL,
    PRIMARY KEY (blocked_id, blocker_id),
    FOREIGN KEY (blocked_id) REFERENCES block_raw(id) ON DELETE CASCADE,
    FOREIGN KEY (blocker_id) REFERENCES block_raw(id) ON DELETE CASCADE
);

-- [actor_ddl]
CREATE TABLE IF NOT EXISTS block_tags (
    block_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (block_id, tag),
    FOREIGN KEY (block_id) REFERENCES block_raw(id) ON DELETE CASCADE
);

-- [actor_ddl]
CREATE MATERIALIZED VIEW block AS -- The `block` matview: hydrates the block_raw rows with the
SELECT
    b.id,
    b.parent_id,
    b.depth,
    b.sort_key,
    b.content,
    b.content_type,
    b.source_language,
    b.source_name,
    b.properties,
    b.marks,
    b.collapsed,
    b.completed,
    b.block_type,
    b.created_at,
    b.updated_at,
    b._change_origin,
    COALESCE(json_group_array(bt.tag)        FILTER (WHERE bt.tag        IS NOT NULL), '[]') AS tags,
    COALESCE(json_group_array(tb.blocker_id) FILTER (WHERE tb.blocker_id IS NOT NULL), '[]') AS blocked_by
FROM block_raw b
LEFT OUTER JOIN block_tags    bt ON bt.block_id   = b.id
LEFT OUTER JOIN task_blockers tb ON tb.blocked_id = b.id
GROUP BY
    b.id,
    b.parent_id,
    b.depth,
    b.sort_key,
    b.content,
    b.content_type,
    b.source_language,
    b.source_name,
    b.properties,
    b.marks,
    b.collapsed,
    b.completed,
    b.block_type,
    b.created_at,
    b.updated_at,
    b._change_origin;

-- [transaction_stmt]
INSERT OR IGNORE INTO block_raw ("parent_id", "created_at", "updated_at", "sort_key", "content", "id", "content_type", "properties") VALUES ('block:ref-doc-0', 1778174723926, 1778174723926, '817F80', 'Dple6 lJaGjrHy3 4b', 'block:2u671h3', 'text', '{"ID":"2u671h3","sequence":3}');

-- [actor_exec]
UPDATE block_raw SET "content" = 'D' WHERE id = 'block:2u671h3';

-- [actor_exec]
UPDATE block_raw SET "content" = 'D' WHERE id = 'block:2u671h3';

-- ?ASSERT ROW-EXISTS block_raw 'block:2u671h3'
-- ?ASSERT ROW-EXISTS block 'block:2u671h3'
-- ?ASSERT ROW-COUNT block 1
