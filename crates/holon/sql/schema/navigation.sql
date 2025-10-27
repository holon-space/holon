CREATE TABLE IF NOT EXISTS navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT,
    timestamp TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_navigation_history_region
ON navigation_history(region);

CREATE TABLE IF NOT EXISTS navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

DROP VIEW IF EXISTS focus_roots;

DROP VIEW IF EXISTS current_focus;

CREATE MATERIALIZED VIEW current_focus AS
SELECT
    nc.region,
    nh.block_id,
    nh.timestamp
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;

-- Resolves focus targets to block IDs. A focus target can be either:
-- - A document URI (doc:xxx) -> root_id = direct children of that document
-- - A block URI (block:xxx) -> root_id = the block itself
-- UNION ALL produces both so downstream queries get a simple equality join.
CREATE MATERIALIZED VIEW focus_roots AS
SELECT cf.region, cf.block_id, b.id AS root_id
FROM current_focus AS cf
JOIN block AS b ON b.parent_id = cf.block_id
UNION ALL
SELECT cf.region, cf.block_id, b.id AS root_id
FROM current_focus AS cf
JOIN block AS b ON b.id = cf.block_id;
